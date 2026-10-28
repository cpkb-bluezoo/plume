/*
 * async_bridge.rs
 * Copyright (C) 2026 Chris Burdess
 *
 * This file is part of Plume, a Nostr desktop client.
 *
 * Plume is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * Plume is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with Plume.  If not, see <http://www.gnu.org/licenses/>.
 */

//! A minimal, `std`-only bridge from hopf's callback-driven reactor threads
//! back into `async fn` Tauri commands -- no tokio. hopf's own networking
//! runs entirely on its own OS threads and delivers results via plain
//! callbacks; `OneshotFuture` lets an `async fn` `.await` one of those
//! results without this crate depending on an async runtime at all (Tauri's
//! own command dispatcher supplies whatever executor actually polls it).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

/// Runs `fut` to completion on a dedicated OS thread -- the `std`-only
/// stand-in for `tokio::spawn`. The futures this codebase builds (dialing
/// through Tor, waiting on a WebSocket `run()`) do their real work on
/// hopf's own reactor threads or dedicated worker threads regardless of
/// whether anything polls them; this thread's only job is to wake up and
/// re-poll on each `Waker::wake()` until the future resolves.
pub fn spawn<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    std::thread::spawn(move || block_on(fut));
}

/// Blocks the calling thread until `fut` resolves, parking between wakeups
/// instead of busy-polling.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = Box::pin(fut);
    let parker = Arc::new(Parker { state: Mutex::new(false), condvar: Condvar::new() });
    let waker = Waker::from(Arc::clone(&parker));
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => parker.park(),
        }
    }
}

struct Parker {
    state: Mutex<bool>,
    condvar: Condvar,
}

impl Parker {
    fn park(&self) {
        let mut woken = self.state.lock().unwrap();
        while !*woken {
            woken = self.condvar.wait(woken).unwrap();
        }
        *woken = false;
    }
}

impl Wake for Parker {
    fn wake(self: Arc<Self>) {
        *self.state.lock().unwrap() = true;
        self.condvar.notify_one();
    }
}

enum State<T> {
    Pending(Option<Waker>),
    Ready(T),
    Taken,
}

/// The receiving half of a one-shot value handed over from a callback
/// running on some other thread (typically a hopf reactor thread).
pub struct OneshotFuture<T> {
    state: Arc<Mutex<State<T>>>,
}

/// The sending half -- call [`Self::send`] exactly once, from any thread.
pub struct OneshotSender<T> {
    state: Arc<Mutex<State<T>>>,
}

/// A fresh one-shot pair.
pub fn oneshot<T>() -> (OneshotSender<T>, OneshotFuture<T>) {
    let state = Arc::new(Mutex::new(State::Pending(None)));
    (OneshotSender { state: Arc::clone(&state) }, OneshotFuture { state })
}

impl<T> OneshotSender<T> {
    /// Deliver the value and wake whoever is awaiting it. A send after the
    /// future was already dropped (nobody awaiting any more) is a no-op.
    pub fn send(self, value: T) {
        let mut guard = self.state.lock().unwrap();
        if let State::Pending(waker) = std::mem::replace(&mut *guard, State::Ready(value)) {
            if let Some(w) = waker {
                w.wake();
            }
        }
    }
}

/// An async counting semaphore -- `std`-only stand-in for
/// `tokio::sync::Semaphore`. Bounds how many callers may hold a permit at
/// once; anyone over the limit awaits until one is released rather than
/// being turned away, so a burst of far more concurrent callers than the
/// cap just queues up and drains through it instead of failing outright.
pub struct Semaphore {
    state: Mutex<SemaphoreState>,
}

struct SemaphoreState {
    available: usize,
    waiters: std::collections::VecDeque<OneshotSender<()>>,
}

/// Holds one permit; releases it (waking the next waiter, if any) on drop.
pub struct SemaphorePermit<'a> {
    semaphore: &'a Semaphore,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Self { state: Mutex::new(SemaphoreState { available: permits, waiters: std::collections::VecDeque::new() }) }
    }

    /// Waits for a permit to become available, queuing (FIFO) behind
    /// anyone already waiting if the semaphore is currently exhausted.
    pub async fn acquire(&self) -> SemaphorePermit<'_> {
        let pending = {
            let mut guard = self.state.lock().unwrap();
            if guard.available > 0 {
                guard.available -= 1;
                None
            } else {
                let (tx, rx) = oneshot::<()>();
                guard.waiters.push_back(tx);
                Some(rx)
            }
        };
        if let Some(rx) = pending {
            rx.await;
        }
        SemaphorePermit { semaphore: self }
    }
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        let mut guard = self.semaphore.state.lock().unwrap();
        match guard.waiters.pop_front() {
            // Ownership of this permit transfers directly to the next
            // waiter -- `available` doesn't change.
            Some(tx) => tx.send(()),
            None => guard.available += 1,
        }
    }
}

impl<T> Future for OneshotFuture<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut guard = self.state.lock().unwrap();
        match &mut *guard {
            State::Ready(_) => {
                let State::Ready(value) = std::mem::replace(&mut *guard, State::Taken) else {
                    unreachable!()
                };
                Poll::Ready(value)
            }
            State::Pending(waker) => {
                *waker = Some(cx.waker().clone());
                Poll::Pending
            }
            State::Taken => panic!("OneshotFuture polled after it already resolved"),
        }
    }
}

/// Elapsed error for [`timeout`], matching `tokio::time::error::Elapsed`'s
/// role without the dependency.
#[derive(Debug)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "deadline elapsed")
    }
}

impl std::error::Error for Elapsed {}

/// Race `fut` against a plain `std::thread::sleep`-backed deadline timer.
/// `std`-only equivalent of `tokio::time::timeout`.
pub async fn timeout<T>(duration: Duration, fut: impl Future<Output = T>) -> Result<T, Elapsed> {
    let (tx, deadline) = oneshot::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        tx.send(());
    });
    Race { fut, deadline }.await
}

/// Resolves to whichever of `fut` / `deadline` completes first, polling
/// both every time either's waker fires.
struct Race<F, D> {
    fut: F,
    deadline: D,
}

impl<T, F: Future<Output = T>, D: Future<Output = ()>> Future for Race<F, D> {
    type Output = Result<T, Elapsed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<T, Elapsed>> {
        // Safety: `Race` is only ever used behind a local `let fut = Race{..}; fut.await`
        // (never moved after first poll), so projecting pinned fields here is sound.
        let this = unsafe { self.get_unchecked_mut() };
        let fut = unsafe { Pin::new_unchecked(&mut this.fut) };
        if let Poll::Ready(value) = fut.poll(cx) {
            return Poll::Ready(Ok(value));
        }
        let deadline = unsafe { Pin::new_unchecked(&mut this.deadline) };
        if deadline.poll(cx).is_ready() {
            return Poll::Ready(Err(Elapsed));
        }
        Poll::Pending
    }
}

/// `std`-only unbounded multi-producer/single-consumer channel with an
/// awaitable `recv()` -- the one piece of `tokio::sync::mpsc` this codebase
/// actually used, reimplemented without the dependency. Mirrors tokio's
/// naming (`unbounded_channel`, `UnboundedSender`, `UnboundedReceiver`) so
/// call sites built against it barely change.
pub mod mpsc {
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    struct State<T> {
        queue: VecDeque<T>,
        waker: Option<Waker>,
        sender_count: usize,
    }

    /// The sending half. Cloneable; the channel closes once every clone is dropped.
    pub struct UnboundedSender<T> {
        state: Arc<Mutex<State<T>>>,
    }

    /// The receiving half.
    pub struct UnboundedReceiver<T> {
        state: Arc<Mutex<State<T>>>,
    }

    /// A fresh unbounded channel.
    pub fn unbounded_channel<T>() -> (UnboundedSender<T>, UnboundedReceiver<T>) {
        let state = Arc::new(Mutex::new(State { queue: VecDeque::new(), waker: None, sender_count: 1 }));
        (UnboundedSender { state: Arc::clone(&state) }, UnboundedReceiver { state })
    }

    impl<T> Clone for UnboundedSender<T> {
        fn clone(&self) -> Self {
            self.state.lock().unwrap().sender_count += 1;
            Self { state: Arc::clone(&self.state) }
        }
    }

    impl<T> Drop for UnboundedSender<T> {
        fn drop(&mut self) {
            let mut guard = self.state.lock().unwrap();
            guard.sender_count -= 1;
            if guard.sender_count == 0 {
                if let Some(w) = guard.waker.take() {
                    w.wake();
                }
            }
        }
    }

    /// Error returned by [`UnboundedSender::send`] when every receiver has
    /// already been dropped (mirrors `tokio::sync::mpsc::error::SendError`'s role).
    #[derive(Debug)]
    pub struct SendError<T>(pub T);

    impl<T> UnboundedSender<T> {
        /// Queue `value`. Never blocks; safe to call from any thread,
        /// including a hopf reactor thread.
        pub fn send(&self, value: T) -> Result<(), SendError<T>> {
            let mut guard = self.state.lock().unwrap();
            guard.queue.push_back(value);
            if let Some(w) = guard.waker.take() {
                w.wake();
            }
            Ok(())
        }
    }

    impl<T> UnboundedReceiver<T> {
        /// Await the next value, or `None` once every sender has been dropped.
        pub fn recv(&mut self) -> RecvFuture<'_, T> {
            RecvFuture { state: &self.state }
        }
    }

    /// Future returned by [`UnboundedReceiver::recv`].
    pub struct RecvFuture<'a, T> {
        state: &'a Arc<Mutex<State<T>>>,
    }

    impl<T> Future for RecvFuture<'_, T> {
        type Output = Option<T>;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
            let mut guard = self.state.lock().unwrap();
            if let Some(value) = guard.queue.pop_front() {
                return Poll::Ready(Some(value));
            }
            if guard.sender_count == 0 {
                return Poll::Ready(None);
            }
            guard.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

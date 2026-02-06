/**
 * Plume i18n – standards-based MessageFormat-style localisation.
 * Supports {variable} replacement. Locale files are nested JSON; keys use dot notation (e.g. nav.home).
 */
(function (global) {
    'use strict';

    const STORAGE_KEY = 'plume:locale';
    const SUPPORTED = ['en', 'fr', 'de', 'es', 'it'];
    const DEFAULT = 'en';

    let messages = {};
    let currentLocale = DEFAULT;

    function getStored() {
        try {
            const s = localStorage.getItem(STORAGE_KEY);
            return s && SUPPORTED.includes(s) ? s : null;
        } catch (_) {
            return null;
        }
    }

    function fromNavigator() {
        const lang = (navigator.language || navigator.userLanguage || '').slice(0, 2).toLowerCase();
        return SUPPORTED.includes(lang) ? lang : DEFAULT;
    }

    function resolve(obj, path) {
        return path.split('.').reduce(function (o, k) {
            return o != null && typeof o === 'object' ? o[k] : undefined;
        }, obj);
    }

    /**
     * Format a message with MessageFormat-style {variable} replacement.
     * @param {string} key - Dot-path key (e.g. 'nav.home', 'composeModal.charCount')
     * @param {Object} [vars] - Optional map of variable names to values for {name} replacement
     * @returns {string} Translated string, or key if missing
     */
    function t(key, vars) {
        let str = resolve(messages, key);
        if (str == null || typeof str !== 'string') {
            return key;
        }
        if (vars && typeof vars === 'object') {
            Object.keys(vars).forEach(function (name) {
                str = str.replace(new RegExp('\\{' + name + '\\}', 'g'), String(vars[name]));
            });
        }
        return str;
    }

    /**
     * Set the current locale and load its messages. Returns a Promise that resolves when ready.
     * @param {string} locale - Locale code (en, fr, de, es, it)
     */
    function setLocale(locale) {
        if (!SUPPORTED.includes(locale)) {
            locale = DEFAULT;
        }
        currentLocale = locale;
        try {
            localStorage.setItem(STORAGE_KEY, locale);
        } catch (_) {}
        return loadMessages(locale);
    }

    function loadMessages(locale) {
        return new Promise(function (resolve, reject) {
            const xhr = new XMLHttpRequest();
            xhr.open('GET', 'locales/' + locale + '.json', true);
            xhr.onload = function () {
                if (xhr.status >= 200 && xhr.status < 300) {
                    try {
                        messages = JSON.parse(xhr.responseText);
                        resolve();
                    } catch (e) {
                        reject(e);
                    }
                } else {
                    if (locale !== DEFAULT) {
                        loadMessages(DEFAULT).then(resolve).catch(reject);
                    } else {
                        reject(new Error('Failed to load locale'));
                    }
                }
            };
            xhr.onerror = function () {
                if (locale !== DEFAULT) {
                    loadMessages(DEFAULT).then(resolve).catch(reject);
                } else {
                    reject(new Error('Network error loading locale'));
                }
            };
            xhr.send();
        });
    }

    function getLocale() {
        return currentLocale;
    }

    function getSupportedLocales() {
        return SUPPORTED.slice();
    }

    /**
     * Apply translations to the document: [data-i18n], [data-i18n-title], [data-i18n-aria-label], [data-i18n-alt], [data-i18n-placeholder].
     */
    function applyI18n() {
        document.documentElement.lang = currentLocale;

        document.querySelectorAll('[data-i18n]').forEach(function (el) {
            const key = el.getAttribute('data-i18n');
            const val = t(key);
            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
                if (el.type === 'submit' || el.type === 'button') {
                    el.value = val;
                }
            } else {
                el.textContent = val;
            }
        });

        document.querySelectorAll('[data-i18n-placeholder]').forEach(function (el) {
            el.placeholder = t(el.getAttribute('data-i18n-placeholder'));
        });

        document.querySelectorAll('[data-i18n-title]').forEach(function (el) {
            el.title = t(el.getAttribute('data-i18n-title'));
        });

        document.querySelectorAll('[data-i18n-aria-label]').forEach(function (el) {
            el.setAttribute('aria-label', t(el.getAttribute('data-i18n-aria-label')));
        });

        document.querySelectorAll('[data-i18n-alt]').forEach(function (el) {
            el.alt = t(el.getAttribute('data-i18n-alt'));
        });
    }

    /**
     * Initialize: determine locale, load messages, apply to document. Call once on page load.
     * @returns {Promise<void>}
     */
    function init() {
        currentLocale = getStored() || fromNavigator();
        return loadMessages(currentLocale).then(applyI18n);
    }

    global.PlumeI18n = {
        t: t,
        setLocale: setLocale,
        getLocale: getLocale,
        getSupportedLocales: getSupportedLocales,
        init: init,
        applyI18n: applyI18n
    };
})(typeof window !== 'undefined' ? window : this);

const QcoldTelegram = (() => {
  const sdkSrc = 'https://telegram.org/js/telegram-web-app.js?62';
  const launchParamPattern = /(?:^|[?&#])tgWebApp(?:Data|Version|Platform|ThemeParams|StartParam)=/;
  const callbacks = [];
  let sdkLoadStarted = false;
  let sdkLoadFailed = false;

  function webApp() {
    return window.Telegram?.WebApp || null;
  }

  function launchParamSource() {
    const hash = window.location.hash ? window.location.hash.slice(1) : '';
    return `${window.location.search || ''}&${hash}`;
  }

  function inLaunchContext() {
    return launchParamPattern.test(launchParamSource());
  }

  function safeCall(app, method, ...args) {
    if (typeof app?.[method] === 'function') app[method](...args);
  }

  function flushCallbacks() {
    const app = webApp();
    if (!app) return;
    while (callbacks.length) callbacks.shift()(app);
  }

  function loadSdkIfNeeded() {
    if (webApp()) {
      flushCallbacks();
      return true;
    }
    if (sdkLoadStarted || sdkLoadFailed || !inLaunchContext()) return false;
    sdkLoadStarted = true;

    const script = document.createElement('script');
    script.async = true;
    script.src = sdkSrc;
    script.onload = flushCallbacks;
    script.onerror = () => {
      sdkLoadFailed = true;
      callbacks.length = 0;
    };
    document.head.appendChild(script);
    return true;
  }

  function whenAvailable(callback) {
    const app = webApp();
    if (app) {
      callback(app);
      return;
    }
    if (inLaunchContext() && !sdkLoadFailed) callbacks.push(callback);
    loadSdkIfNeeded();
  }

  function readyAndExpand() {
    whenAvailable((app) => {
      safeCall(app, 'ready');
      safeCall(app, 'expand');
    });
  }

  function applyTheme(choice) {
    whenAvailable((app) => {
      safeCall(app, 'setHeaderColor', choice === 'dark' ? '#101114' : 'secondary_bg_color');
      safeCall(app, 'setBackgroundColor', choice === 'dark' ? '#101114' : 'bg_color');
    });
  }

  function showAlert(message) {
    whenAvailable((app) => {
      safeCall(app, 'showAlert', message);
    });
  }

  loadSdkIfNeeded();

  return {
    webApp,
    inLaunchContext,
    loadSdkIfNeeded,
    readyAndExpand,
    applyTheme,
    showAlert,
  };
})();

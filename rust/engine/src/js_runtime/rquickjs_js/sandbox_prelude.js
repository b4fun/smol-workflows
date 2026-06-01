// Temporary bootstrap helper. Rust captures this function during runtime setup
// and then removes globalThis.__readonly before user workflow code runs.
globalThis.__readonly = (() => {
  const cache = new WeakMap();

  function readonly(value) {
    if (typeof value !== 'object' || value === null) return value;
    const cached = cache.get(value);
    if (cached) return cached;

    const proxy = new Proxy(value, {
      get(target, property, receiver) { return readonly(Reflect.get(target, property, receiver)); },
      set() { throw new TypeError('Cannot modify workflow value'); },
      defineProperty() { throw new TypeError('Cannot modify workflow value'); },
      deleteProperty() { throw new TypeError('Cannot modify workflow value'); },
      setPrototypeOf() { throw new TypeError('Cannot modify workflow value'); },
    });
    cache.set(value, proxy);
    return proxy;
  }

  return readonly;
})();

Object.defineProperty(globalThis, 'parallel', {
  value: async function parallel(tasks) {
    return await Promise.all(tasks.map(async (task) => {
      try { return await task(); } catch { return null; }
    }));
  },
  enumerable: true,
  writable: false,
  configurable: false,
});

Object.defineProperty(globalThis, 'pipeline', {
  value: async function pipeline(items, ...stages) {
    return await Promise.all(items.map(async (item, index) => {
      let previous = item;
      for (const stage of stages) {
        try { previous = await stage(previous, item, index); } catch { return null; }
      }
      return previous;
    }));
  },
  enumerable: true,
  writable: false,
  configurable: false,
});



# JavaScript Semaphore

A promise-based semaphore implementation suitable to be used with async/await.

## Spare me the details, all I need is a lock.
Just ```import { Lock } from 'semaphore-async-await'```, acquire the lock by calling ```await lock.acquire()``` and release it when you're done by calling ```lock.release()```.

## But JavaScript is single-threaded and doesn't need semaphores!
This package can be used to synchronize functions that span multiple iterations of the event loop and prevent other code from being executed while your function is waiting.

Suppose you have a function

```javascript
async function criticalFunction() {
  const data = await getDataFromDb();
  const modifiedData = await asynchronouslyDoStuffWithData(data);
  return writeDataBackToDb(modifiedData);
}
```

Calling this function repeatedly could lead to overlapping read/writes. To avoid this problem, a lock can be added like so:

```javascript
const lock = new Semaphore(1);

async function criticalFunctionSynchronous() {
  await lock.acquire();

  await criticalFunction();

  lock.release();
}
```

Asynchronous functions like ```criticalFunction``` are executed in multiple chunks of code on the event loop, this package makes it possible to enforce an ordering in these chunks.

## Install
```yarn add semaphore-async-await```

[<h2>API</h2>](http://jsoendermann.github.io/semaphore-async-await/classes/semaphore.html)

## Usage
```javascript
import Semaphore from 'semaphore-async-await';

(async () => {

  // A Semaphore with one permit is a lock
  const lock = new Semaphore(1);

  // Helper function used to wait for the given number of milliseconds
  const wait = (ms) => new Promise(r => setTimeout(r, ms));

  let globalVar = 0;

  (async () => {
    // This waits (without blocking the event loop) until a permit becomes available
    await lock.wait();
    const localCopy = globalVar;
    await wait(500);
    globalVar = localCopy + 1;
    // Signal releases the lock and lets other things run
    lock.signal();
  })();

  // This returns false because the function above has acquired the lock
  // and is scheduled to continue executing once the main function yields or
  // returns
  console.log(lock.tryAcquire() === false);

  // Similar to the function above but using waitFor instead of wait. We
  // give it five seconds to wait which is enough time for it to acquire
  // the lock
  (async () => {
    // This waits for at least five seconds, trying to acquire a permit.
    const didAcquireLock = await lock.waitFor(5000);
    if (didAcquireLock) {
      const localCopy = globalVar;
      await wait(500);
      globalVar = localCopy + 1;
      // Signal releases the lock and lets other things run
      lock.signal();
    }
  })();

  // Alternative to using wait()/signal() directly
  lock.execute(async () => {
    const localCopy = globalVar;
    await wait(500);
    globalVar = localCopy + 1;
  });

  // Wait for everything to finish
  await wait(2000);

  console.log(globalVar === 3);
})();
```

## License
MIT

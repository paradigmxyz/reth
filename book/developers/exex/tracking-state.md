# Tracking State

In this chapter, we'll learn how to keep track of some state inside our ExEx.

Let's continue with our Hello World example from the [previous chapter](./hello-world.md).

### Turning ExEx into a struct

First, we need to turn our ExEx into a stateful struct.

Before, we had just an async function, but now we'll need to implement
the [`Future`](https://doc.rust-lang.org/std/future/trait.Future.html) trait manually.

<div class="warning">

Having a stateful async function is also possible, but it makes testing harder,
because you can't access variables inside the function to assert the state of your ExEx.

</div>

```rust,norun,noplayground,ignore
{{#include ../../sources/exex/tracking-state/src/bin/1.rs}}
```

For those who are not familiar with how async Rust works on a lower level, that may seem scary,
but let's unpack what's going on here:

1. Our ExEx is now a `struct` that contains the context and implements the `Future` trait. It's now pollable (hence `await`-able).
1. We can't use `self` directly inside our `poll` method, and instead need to acquire a mutable reference to the data inside of the `Pin`.
   Read more about pinning in [the book](https://rust-lang.github.io/async-book/04_pinning/01_chapter.html).
1. We also can't use `await` directly inside `poll`, and instead need to poll futures manually.
   We wrap the call to `poll_recv(cx)` into a [`ready!`](https://doc.rust-lang.org/std/task/macro.ready.html) macro,
   so that if the channel of notifications has no value ready, we will instantly return `Poll::Pending` from our Future.
1. We initialize and return the `MyExEx` struct directly in the `install_exex` method, because it's a Future.

With all that done, we're now free to add more fields to our `MyExEx` struct, and track some state in them.

### Adding state

Our ExEx will count the number of transactions in each block and log it to the console.

```rust,norun,noplayground,ignore
{{#include ../../sources/exex/tracking-state/src/bin/2.rs}}
```

As you can see, we added two fields to our ExEx struct:
- `first_block` to keep track of the first block that was committed since the start of the ExEx.
- `transactions` to keep track of the total number of transactions committed, accounting for reorgs and reverts.

We also changed our `match` block to two `if` clauses:
- First one checks if there's a reverted chain using `notification.reverted_chain()`. If there is:
    - We subtract the number of transactions in the reverted chain from the total number of transactions.
    - It's important to do the `saturating_sub` here, because if we just started our node and
      instantly received a reorg, our `transactions` field will still be zero.
- Second one checks if there's a committed chain using `notification.committed_chain()`. If there is:
    - We update the `first_block` field to the first block of the committed chain.
    - We add the number of transactions in the committed chain to the total number of transactions.
    - We send a `FinishedHeight` event back to the main node.

Finally, on every notification, we log the total number of transactions and
the first block that was committed since the start of the ExEx.

A simple async executor
===

A simple executor that runs `Future`s asynchronously on a separate thread
for learning purposes.

## How it works

`Executor::new()` spawns a detached worker thread that stores the receiver end
of an MPSC queue, returning the user a type containing the `Sender` end.
`Executor::spawn(Future)` creates a `Task(Future, Sender)` that is sent via the
MSPC to the worker thread. The worker thread continuously `poll`s the `Future`s
of the `Task`s arriving through the MPSC queue. If the `Future` isn't ready, its
`Waker` is set up to re-schedule itself into the MPSC queue when progress can be
made.

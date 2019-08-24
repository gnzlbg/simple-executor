//! A simple executor that runs Futures asynchronously in a separate thread

use std::{
    cell::UnsafeCell,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use crossbeam_channel::{Sender, Receiver};

/// Executes tasks in a separate thread.
pub struct Executor {
    scheduler: Option<Sender<Arc<Task>>>,
    stopped: Receiver<()>,
}

impl Executor {
    /// Constructor
    pub fn new() -> Arc<Executor> {
        let (s, r) = crossbeam_channel::unbounded::<Arc<Task>>();
        let (s2, r2) = crossbeam_channel::bounded::<()>(1);
        std::thread::spawn(move || {
            while let Ok(task) = r.recv() {
                task.run()
            }
            s2.send(()).unwrap();
        });
        Arc::new(Executor { scheduler: Some(s), stopped: r2 })
    }
    /// Spawns `future` in the executor:
    pub fn spawn<F>(&self, future: F)
        where
        F: Future<Output = ()> + 'static + Send,
    {
        let task = Arc::new(Task {
            future: Some(UnsafeCell::new(Box::pin(future))),
            scheduler: self.scheduler.as_ref().unwrap().clone(),
        });
        self.scheduler.as_ref().unwrap().send(task).unwrap();
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        drop(self.scheduler.take().expect("failed to drop Executor"));
        self.stopped.recv().unwrap();
    }
}

struct Task {
    future: Option<UnsafeCell<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>>,
    scheduler: Sender<Arc<Task>>,
}
unsafe impl Send for Task {} // Safe: tasks are only accessed from same thread
unsafe impl Sync for Task {} // Safe: tasks are only accessed from same thread

impl Task {
    fn run(self: Arc<Task>) {
        assert!(Arc::strong_count(&self) == 1 && Arc::weak_count(&self) == 0);
        // Safe: only one Arc to the Task exists here:
        let task: &mut Task = unsafe { &mut *(Arc::into_raw(self) as *mut Task) };
        if let Some(future) = task.future.take() {
            let waker = task.waker();
            let context = &mut Context::from_waker(&waker);
            // Safe: the UnsafeCell is only accessed from same thread:
            if let Poll::Pending = (unsafe { &mut *future.get() }).as_mut().poll(context) {
                task.future = Some(future);
            }
        }
    }

    fn waker(&self) -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
        unsafe fn drop(x: *const ()) { Arc::from_raw(x as *const Task); }
        unsafe fn clone(x: *const ()) -> RawWaker {
            let task = Arc::from_raw(x as *const Task);
            let x = Arc::into_raw(Arc::clone(&task)) as *const ();
            let _ = Arc::into_raw(task);
            RawWaker::new(x, &VTABLE)
        }
        unsafe fn wake_by_ref(x: *const ()) {
            let task = Arc::from_raw(x as *const Task);
            let x = Arc::into_raw(Arc::clone(&task)) as *const ();
            let _ = Arc::into_raw(task);
            wake(x);
        }
        unsafe fn wake(x: *const ()) {
            let ptr = x as *const Task;
            (*ptr)
                .scheduler
                .send(Arc::from_raw(ptr))
                .expect("failed to re-schedule task");
        }

        let raw_waker = RawWaker::new(self as *const _ as *const (), &VTABLE);
        unsafe { Waker::from_raw(raw_waker) }
    }
}

cfg_if::cfg_if! {
    if #[cfg(test)] {
        use std::sync::atomic::{Ordering, AtomicUsize};
        static TASK_DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);
        impl Drop for Task {
            fn drop(&mut self) {
                TASK_DROP_COUNTER.fetch_add(1, Ordering::SeqCst);
            }
        }
        #[test]
        fn test() {
            static TEST_FUTURE_READY_COUNTER: AtomicUsize = AtomicUsize::new(0);
            static TEST_FUTURE_PENDING_COUNTER: AtomicUsize = AtomicUsize::new(0);
            static TEST_FUTURE_DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);

            struct DropGuard;
            impl Drop for DropGuard {
                fn drop(&mut self) {
                    assert_eq!(TASK_DROP_COUNTER.load(Ordering::SeqCst), 2);
                    assert_eq!(TEST_FUTURE_DROP_COUNTER.load(Ordering::SeqCst), 2);
                    assert_eq!(TEST_FUTURE_PENDING_COUNTER.load(Ordering::SeqCst), 6);
                    assert_eq!(TEST_FUTURE_READY_COUNTER.load(Ordering::SeqCst), 2);
                }
            }

            struct TestFuture(usize);
            impl Future for TestFuture {
            type Output = ();
                fn poll(mut self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
                    if self.as_mut().0 == 0 {
                        TEST_FUTURE_READY_COUNTER.fetch_add(1, Ordering::SeqCst);
                        Poll::Ready(())
                    } else {
                        TEST_FUTURE_PENDING_COUNTER.fetch_add(1, Ordering::SeqCst);
                        self.as_mut().0 -= 1;
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        ctx.waker().wake_by_ref();
                        Poll::Pending
                    }
                }
            }
            impl Drop for TestFuture {
                fn drop(&mut self) {
                    TEST_FUTURE_DROP_COUNTER.fetch_add(1, Ordering::SeqCst);
                }
            }

            async fn test_fn() -> () {
                let x = TestFuture(3);
                x.await;
            }

            TASK_DROP_COUNTER.store(0, Ordering::SeqCst);
            let _g = DropGuard;
            let e = Executor::new();
            let e2 = e.clone();
            e2.spawn(test_fn());
            std::thread::spawn(move || e.spawn(test_fn())).join().unwrap();
        }
    }
}

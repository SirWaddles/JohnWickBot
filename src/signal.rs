extern crate ctrlc;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use futures::{Poll, Future, Async, Stream};
use tokio::timer;

pub struct TerminationFuture {
    check_timer: timer::Interval,
    exit_bool: Arc<AtomicBool>,
}

impl TerminationFuture {
    pub fn new() -> Self {
        let exit = Arc::new(AtomicBool::new(false));
        let r = exit.clone();
        ctrlc::set_handler(move || {
            r.store(true, Ordering::SeqCst);
        }).unwrap();

        Self {
            check_timer: timer::Interval::new_interval(Duration::from_secs(1)),
            exit_bool: exit,
        }
    }
}

impl Future for TerminationFuture {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        loop {
            match self.check_timer.poll() {
                Ok(Async::Ready(_)) => {
                    if self.exit_bool.load(Ordering::SeqCst) == true {
                        return Ok(Async::Ready(()));
                    } else {
                        return Ok(Async::NotReady);
                    }
                },
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Err(_) => return Err(()),
            };
        }
    }
}
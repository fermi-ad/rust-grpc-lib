//! One fixed-cadence scheduling probe per active telemetry pipeline.

use std::time::Duration;

use opentelemetry::metrics::Histogram;
use tokio::{
    sync::watch,
    time::{Instant, sleep_until},
};

use super::Telemetry;

#[cfg(test)]
mod tests;

const PERIOD: Duration = Duration::from_millis(10);

/// Keeps a server counted until its shutdown signal or the serve future drops.
pub(crate) struct Serving(pub(super) Telemetry);

impl Drop for Serving {
    fn drop(&mut self) {
        let mut lifecycle = self.0.inner.lifecycle.lock().unwrap();
        if let Some(running) = &mut lifecycle.probe {
            running.servers -= 1;
            if running.servers == 0 {
                drop(lifecycle.probe.take());
            }
        }
    }
}

pub(super) struct Running {
    pub(super) servers: usize,
    stop: watch::Sender<bool>,
}

impl Running {
    pub(super) fn start(histogram: Histogram<f64>) -> Self {
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(sample(histogram, stopped, Instant::now() + PERIOD));
        Self { servers: 0, stop }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // Any sample already in progress finishes before this returns. The
        // detached task cannot record again, even if it is not polled until
        // after the provider has begun shutting down.
        self.stop.send_replace(true);
    }
}

async fn sample(
    histogram: Histogram<f64>,
    mut stopped: watch::Receiver<bool>,
    mut deadline: Instant,
) {
    loop {
        tokio::select! {
            biased;
            _ = stopped.changed() => return,
            _ = sleep_until(deadline) => {
                let stopping = stopped.borrow();
                if *stopping {
                    return;
                }
                let now = Instant::now();
                let delay = now.saturating_duration_since(deadline);
                histogram.record(delay.as_secs_f64(), &[]);
                deadline = if delay >= PERIOD { now } else { deadline } + PERIOD;
            }
        }
    }
}

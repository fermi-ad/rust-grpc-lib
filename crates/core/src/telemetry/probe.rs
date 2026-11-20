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

const SAMPLE_PERIOD: Duration = Duration::from_millis(10);

/// Keeps a server counted until its shutdown signal or the serve future drops.
pub(crate) struct ServerGuard(pub(super) Telemetry);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let mut lifecycle = self.0.inner.lifecycle.lock().unwrap();
        let Some(running_probe) = &mut lifecycle.probe else {
            return;
        };
        running_probe.attached_servers -= 1;
        if running_probe.attached_servers == 0 {
            drop(lifecycle.probe.take());
        }
    }
}

pub(super) struct RunningProbe {
    pub(super) attached_servers: usize,
    stop_tx: watch::Sender<bool>,
}

impl RunningProbe {
    pub(super) fn start(histogram: Histogram<f64>) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        tokio::spawn(sample_scheduling_delay(
            histogram,
            stop_rx,
            Instant::now() + SAMPLE_PERIOD,
        ));
        Self {
            attached_servers: 0,
            stop_tx,
        }
    }
}

impl Drop for RunningProbe {
    fn drop(&mut self) {
        // The watch write waits for any sample holding the read guard, then
        // prevents further recording even if the detached task wakes later.
        self.stop_tx.send_replace(true);
    }
}

async fn sample_scheduling_delay(
    histogram: Histogram<f64>,
    mut stop_rx: watch::Receiver<bool>,
    mut deadline: Instant,
) {
    loop {
        tokio::select! {
            biased;
            _ = stop_rx.changed() => return,
            _ = sleep_until(deadline) => {
                let stop_requested = stop_rx.borrow();
                if *stop_requested {
                    return;
                }
                let now = Instant::now();
                let delay = now.saturating_duration_since(deadline);
                histogram.record(delay.as_secs_f64(), &[]);
                deadline = if delay >= SAMPLE_PERIOD { now } else { deadline } + SAMPLE_PERIOD;
            }
        }
    }
}

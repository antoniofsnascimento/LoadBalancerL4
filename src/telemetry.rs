use std::sync::atomic::{AtomicUsize};

#[derive(Debug, Default)]
pub struct BackendStats {
    pub active_connections: AtomicUsize,
    pub total_connections: AtomicUsize,
    pub bytes_transferred: AtomicUsize,
}

pub struct Telemetry {
    pub total_active_connections: AtomicUsize,
    pub total_connections_accepted: AtomicUsize,
    pub total_bytes_transferred: AtomicUsize,
    pub backend_stats: Vec<BackendStats>,
}

impl Telemetry {
    pub fn new(num_backends: usize) -> Self {
        let mut stats = Vec::with_capacity(num_backends);
        for _ in 0..num_backends {
            stats.push(BackendStats::default());
        }
        Self {
            total_active_connections: AtomicUsize::new(0),
            total_connections_accepted: AtomicUsize::new(0),
            total_bytes_transferred: AtomicUsize::new(0),
            backend_stats: stats,
        }
    }
}

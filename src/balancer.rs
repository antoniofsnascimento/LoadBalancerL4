use std::sync::atomic::{AtomicUsize, Ordering};
use crate::models::Backend;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Algorithm {
    RoundRobin,
    WeightedRoundRobin,
}

pub struct LoadBalancer {
    backends: Vec<Backend>,
    algorithm: Algorithm,
    virtual_indices: Vec<usize>,
    current: AtomicUsize,
}

impl LoadBalancer {
    pub fn new(backends: Vec<Backend>, algorithm: Algorithm) -> Self {
        assert!(!backends.is_empty(), "Error: Empty backend list.");
        
        let mut virtual_indices = Vec::new();
        match algorithm {
            Algorithm::RoundRobin => {
                for i in 0..backends.len() {
                    virtual_indices.push(i);
                }
            }
            Algorithm::WeightedRoundRobin => {
                for (i, backend) in backends.iter().enumerate() {
                    for _ in 0..backend.weight {
                        virtual_indices.push(i);
                    }
                }
                // Fallback to Round Robin se os pesos estiverem todos a zero
                if virtual_indices.is_empty() {
                    for i in 0..backends.len() {
                        virtual_indices.push(i);
                    }
                }
            }
        }
        
        Self {
            backends,
            algorithm,
            virtual_indices,
            current: AtomicUsize::new(0),
        }
    }
    
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
    
    pub fn backends(&self) -> &[Backend] {
        &self.backends
    }

    pub fn next_backend(&self) -> (usize, &Backend) {
        // Uso de fetch_add atómico garante segurança multi-thread (lock-free) e máxima velocidade
        let index = self.current.fetch_add(1, Ordering::Relaxed);
        let v_index = self.virtual_indices[index % self.virtual_indices.len()];
        (v_index, &self.backends[v_index])
    }
}

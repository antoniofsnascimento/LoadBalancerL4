use crate::models::Backend;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Algorithm {
    RoundRobin,
    WeightedRoundRobin,
}

impl Algorithm {
    /// Parse an algorithm name from a string (case-insensitive). Accepts:
    ///   "1" | "rr"  | "roundrobin"          -> RoundRobin
    ///   "2" | "wrr" | "weightedroundrobin"  -> WeightedRoundRobin
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "rr" | "roundrobin" | "round_robin" => Some(Self::RoundRobin),
            "2" | "wrr" | "weightedroundrobin" | "weighted_round_robin" => {
                Some(Self::WeightedRoundRobin)
            }
            _ => None,
        }
    }
}

pub struct BalancerState {
    pub backends: Vec<Backend>,
    pub virtual_indices: Vec<usize>,
    pub algorithm: Algorithm,
}

#[derive(Clone)]
pub struct LoadBalancer {
    pub state: Arc<RwLock<BalancerState>>,
    current: Arc<AtomicUsize>,
}

impl LoadBalancer {
    pub fn new(backends: Vec<Backend>, algorithm: Algorithm) -> Self {
        assert!(!backends.is_empty(), "Error: Empty backend list.");

        let state = BalancerState {
            virtual_indices: Self::build_virtual_indices(&backends, algorithm),
            backends,
            algorithm,
        };

        Self {
            state: Arc::new(RwLock::new(state)),
            current: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn build_virtual_indices(backends: &[Backend], algorithm: Algorithm) -> Vec<usize> {
        let mut virtual_indices = Vec::new();
        match algorithm {
            Algorithm::RoundRobin => {
                // Classic RR: each enabled backend appears exactly once.
                // weight==0 means "out of rotation" (operator-disabled OR unhealthy).
                for (i, backend) in backends.iter().enumerate() {
                    if backend.weight > 0 {
                        virtual_indices.push(i);
                    }
                }
            }
            Algorithm::WeightedRoundRobin => {
                for (i, backend) in backends.iter().enumerate() {
                    for _ in 0..backend.weight {
                        virtual_indices.push(i);
                    }
                }
            }
        }
        // Safety net: never produce an empty virtual table — that would panic at
        // `index % 0` in next_backend(). If every backend is disabled we fall
        // back to round-robin over ALL backends so the LB keeps trying instead
        // of crashing. In practice the watchdog should bring at least one back.
        if virtual_indices.is_empty() {
            for i in 0..backends.len() {
                virtual_indices.push(i);
            }
        }
        virtual_indices
    }

    pub fn update_weights(&self, new_weights: Vec<u32>) {
        // Fast path: skip the write-lock if nothing changed.
        {
            let state = self.state.read().unwrap();
            let unchanged = new_weights
                .iter()
                .zip(state.backends.iter())
                .all(|(w, b)| *w == b.weight)
                && new_weights.len() >= state.backends.len();
            if unchanged {
                return;
            }
        }

        let mut state = self.state.write().unwrap();
        for (i, weight) in new_weights.into_iter().enumerate() {
            if i < state.backends.len() {
                state.backends[i].weight = weight;
            }
        }
        let alg = state.algorithm;
        state.virtual_indices = Self::build_virtual_indices(&state.backends, alg);
        self.current.store(0, Ordering::Relaxed);
    }

    /// Live-switch the routing algorithm. Rebuilds the virtual-index table.
    pub fn set_algorithm(&self, new_alg: Algorithm) {
        {
            let state = self.state.read().unwrap();
            if state.algorithm == new_alg {
                return;
            }
        }
        let mut state = self.state.write().unwrap();
        state.algorithm = new_alg;
        state.virtual_indices = Self::build_virtual_indices(&state.backends, new_alg);
        self.current.store(0, Ordering::Relaxed);
    }

    pub fn algorithm(&self) -> Algorithm {
        self.state.read().unwrap().algorithm
    }

    pub fn backends(&self) -> Vec<Backend> {
        let state = self.state.read().unwrap();
        state.backends.clone()
    }

    pub fn next_backend(&self) -> (usize, Backend) {
        let state = self.state.read().unwrap();
        let index = self.current.fetch_add(1, Ordering::Relaxed);
        let len = state.virtual_indices.len();
        let v_index = state.virtual_indices[index % len];
        let backend = state.backends[v_index].clone();
        (v_index, backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_mock_backends() -> Vec<Backend> {
        vec![
            Backend {
                addr: "127.0.0.1:8081".to_string(),
                weight: 3,
            },
            Backend {
                addr: "127.0.0.1:8082".to_string(),
                weight: 1,
            },
            Backend {
                addr: "127.0.0.1:8083".to_string(),
                weight: 1,
            },
        ]
    }

    #[test]
    fn test_round_robin() {
        let backends = create_mock_backends();
        let lb = LoadBalancer::new(backends, Algorithm::RoundRobin);

        let (_, b1) = lb.next_backend();
        assert_eq!(b1.addr, "127.0.0.1:8081");

        let (_, b2) = lb.next_backend();
        assert_eq!(b2.addr, "127.0.0.1:8082");

        let (_, b3) = lb.next_backend();
        assert_eq!(b3.addr, "127.0.0.1:8083");

        let (_, b4) = lb.next_backend();
        assert_eq!(b4.addr, "127.0.0.1:8081");
    }

    #[test]
    fn test_weighted_round_robin() {
        let backends = create_mock_backends();
        let lb = LoadBalancer::new(backends, Algorithm::WeightedRoundRobin);

        // With weights 3, 1, 1 -> virtual_indices should have length 5
        let state = lb.state.read().unwrap();
        assert_eq!(state.virtual_indices.len(), 5);
        drop(state);

        let mut counts = vec![0, 0, 0];
        for _ in 0..5 {
            let (idx, _) = lb.next_backend();
            counts[idx] += 1;
        }
        assert_eq!(counts, vec![3, 1, 1]);
    }

    #[test]
    fn test_update_weights_hot_reload() {
        let backends = create_mock_backends();
        let lb = LoadBalancer::new(backends, Algorithm::WeightedRoundRobin);

        // Change weights to 0, 2, 0
        lb.update_weights(vec![0, 2, 0]);

        let state = lb.state.read().unwrap();
        assert_eq!(state.virtual_indices.len(), 2);
        drop(state);

        let (idx1, b1) = lb.next_backend();
        assert_eq!(idx1, 1);
        assert_eq!(b1.addr, "127.0.0.1:8082");
    }

    #[test]
    fn test_zero_weights_fallback() {
        let backends = create_mock_backends();
        let lb = LoadBalancer::new(backends, Algorithm::WeightedRoundRobin);

        // If all weights are 0, it should fallback to RoundRobin
        lb.update_weights(vec![0, 0, 0]);

        let state = lb.state.read().unwrap();
        assert_eq!(state.virtual_indices.len(), 3);
    }

    #[test]
    fn test_set_algorithm_live_switch() {
        let backends = create_mock_backends();
        let lb = LoadBalancer::new(backends, Algorithm::RoundRobin);

        // Starts as RoundRobin → all backends have weight>0, so length 3.
        assert_eq!(lb.state.read().unwrap().virtual_indices.len(), 3);

        // Switch live.
        lb.set_algorithm(Algorithm::WeightedRoundRobin);
        assert_eq!(lb.algorithm(), Algorithm::WeightedRoundRobin);
        // With weights 3,1,1 the WRR table is length 5.
        assert_eq!(lb.state.read().unwrap().virtual_indices.len(), 5);
    }

    #[test]
    fn test_round_robin_excludes_zero_weight() {
        // In Round Robin, a backend with weight=0 must be treated as offline
        // (out of rotation), exactly like in Weighted Round Robin.
        let backends = vec![
            Backend { addr: "127.0.0.1:8081".to_string(), weight: 1 },
            Backend { addr: "127.0.0.1:8082".to_string(), weight: 0 }, // offline
            Backend { addr: "127.0.0.1:8083".to_string(), weight: 1 },
        ];
        let lb = LoadBalancer::new(backends, Algorithm::RoundRobin);

        // Only 2 backends in rotation.
        assert_eq!(lb.state.read().unwrap().virtual_indices.len(), 2);

        // Verify routing skips backend index 1.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            let (idx, _) = lb.next_backend();
            seen.insert(idx);
        }
        assert!(seen.contains(&0));
        assert!(seen.contains(&2));
        assert!(!seen.contains(&1)); // offline backend never picked
    }

    #[test]
    fn test_algorithm_parse() {
        assert_eq!(Algorithm::parse("rr"), Some(Algorithm::RoundRobin));
        assert_eq!(Algorithm::parse("ROUNDROBIN"), Some(Algorithm::RoundRobin));
        assert_eq!(Algorithm::parse("1"), Some(Algorithm::RoundRobin));
        assert_eq!(Algorithm::parse("wrr"), Some(Algorithm::WeightedRoundRobin));
        assert_eq!(Algorithm::parse("2"), Some(Algorithm::WeightedRoundRobin));
        assert_eq!(Algorithm::parse("???"), None);
    }
}

#[derive(Debug, Clone)]
pub struct Backend {
    pub addr: String, // IP:Port, &str is not used because it does not own the data
    pub weight: u32, // For load balancing algorithms
}
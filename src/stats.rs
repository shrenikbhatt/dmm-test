pub trait MemStats {
    fn calculate_allocation_ratio(&self) -> (f64, f64, f64);
    fn reset(&mut self);
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub line_count: usize,
    pub zone_count: usize,
    pub token_count: usize,
    pub trigram_count: usize,
    pub index_size_bytes: usize,
}

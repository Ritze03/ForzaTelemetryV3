use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct EngineRecord {
    pub engine_label: String,
    pub source_vehicle: String,
    pub engine_name: String,
    pub horsepower: u32,
}

static ENGINES_CSV: &str = include_str!("../assets/engines.csv");

pub fn load_engines() -> Vec<EngineRecord> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(ENGINES_CSV.as_bytes());

    reader.deserialize().filter_map(|r| r.ok()).collect()
}

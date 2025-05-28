use std::collections::{HashMap, HashSet};
use std::fs;
use crate::{
    asns,
    database::{CollectServersFilter, Database},
    scanner::targets::ScanRange,
};

pub async fn get_ranges(database: &Database) -> anyhow::Result<Vec<ScanRange>> {
    let asns = asns::get().await?;

    let contents = fs::read_to_string("asns.txt")?;
    let asns_with_servers: HashSet<u32> = contents
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect();

    let mut ranges = Vec::new();
    for asn in asns_with_servers {
        let asn_ranges = asns.get_ranges_for_asn(asn);
        for range in asn_ranges {
            ranges.push(ScanRange::single_port(range.start, range.end, 25565));
        }
    }

    Ok(ranges)
}

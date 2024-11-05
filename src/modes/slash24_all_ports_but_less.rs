use std::{collections::HashMap, net::Ipv4Addr};

use crate::{
    database::{CollectServersFilter, Database},
    modes::slash24::{to_ranges, ServerGroup},
    scanner::targets::ScanRange,
};

/// Scan ports that are likely to have servers on random ranges.
pub async fn get_ranges(database: &Database) -> anyhow::Result<Vec<ScanRange>> {
    println!("Collecting servers that were active in the last 30 days");
    let known_servers =
        crate::database::collect_all_servers(database, CollectServersFilter::Active30d).await?;
    println!("Collected {} servers in total", known_servers.len());

    let ranges: HashMap<(u8, u8, u8), ServerGroup> = to_ranges(&known_servers);
    println!("Converted them into {} ranges", ranges.len());

    let mut target_ranges = Vec::new();

    for ((a, b, c), range) in ranges {
        if range.ips.len() < 3 || range.ports.len() < 3 {
            continue;
        }
        target_ranges.push(ScanRange {
            addr_start: Ipv4Addr::new(a, b, c, 0),
            addr_end: Ipv4Addr::new(a, b, c, 255),
            port_start: 1024,
            port_end: 65535,
        });
    }

    Ok(target_ranges)
}

use pnet::packet::tcp::TcpOption;

#[derive(Debug, Clone)]
pub struct TcpFingerprint {
    pub initial_ttl: u8,
    pub window_size: u16,
    pub options: Vec<TcpOption>
}

impl TcpFingerprint {
    pub fn parse_signature(sig: &str, mss_arg: u16) -> Self {
        let parts: Vec<&str> = sig.split(":").collect();
        if parts.len() != 8 {
            panic!("Invalid p0f signature specified (expected 8 parts)");
        }

        if parts[0] != "4" && parts[0] != "*" {
            panic!("Invalid p0f signature specified (only IPv4 is supported)");
        }

        let mut mss = mss_arg;
        let initial_ttl = parts[1].parse::<u8>().unwrap();
        if parts[3] != "*" {
            mss = parts[3].parse::<u16>().unwrap();
        }

        let window_size: u16;
        let window_scaling: u8;
        if parts[4].starts_with("mss*") {
            let split: Vec<&str> = parts[4][4..].split(",").collect();
            window_size = mss * split[0].parse::<u16>().unwrap();
            window_scaling = split[1].parse::<u8>().unwrap();
        } else if parts[4].starts_with("mtu*") {
            panic!("mtu*N window sizes are not supported");
        } else if parts[4] == "*" {
            window_size = 0;
            window_scaling = 0;
        } else {
            let split: Vec<&str> = parts[4].split(",").collect();
            window_size = split[0].parse::<u16>().unwrap();
            window_scaling = split[1].parse::<u8>().unwrap();
        }

        let mut options: Vec<TcpOption> = Vec::new();
        let layout: Vec<&str> = parts[5].split(",").collect();
        for item in layout {
            match item {
                "nop" => {
                    options.push(TcpOption::nop());
                },
                "mss" => {
                    options.push(TcpOption::mss(mss));
                },
                "ws" => {
                    options.push(TcpOption::wscale(window_scaling));
                },
                "sok" => {
                    options.push(TcpOption::sack_perm());
                },
                "ts" => {
                    options.push(TcpOption::timestamp(1, 0));
                },
                _ => {}
            }
        }

        Self {
            initial_ttl,
            window_size,
            options
        }
    }
}

impl Default for TcpFingerprint {
    fn default() -> Self {
        // Default signature for Linux 3.11 and newer
        Self::parse_signature("*:64:0:*:mss*20,10:mss,sok,ts,nop,ws:df,id+:0", 1500)
    }
}
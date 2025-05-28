use lazy_static::lazy_static;
use prometheus_exporter::{
    self,
    prometheus::IntCounter,
    prometheus::IntCounterVec,
    prometheus::register_int_counter,
    prometheus::register_int_counter_vec
};

lazy_static! {
    pub static ref SERVERS_FOUND_COUNTER: IntCounterVec =
        register_int_counter_vec!("so_matscan_found", "Number of new servers found", &["mode"]).unwrap();
    pub static ref SERVERS_RESCANNED_COUNTER: IntCounter =
        register_int_counter!("so_matscan_rescanned", "Number of servers rescanned").unwrap();
    pub static ref SERVERS_FINGERPRINTED_COUNTER: IntCounter =
        register_int_counter!("so_matscan_fingerprint", "Number of servers fingerprinted").unwrap();
}
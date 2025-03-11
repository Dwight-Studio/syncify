use spdlog::sink::StdStreamSink;
use spdlog::Logger;
use std::sync::Arc;

pub struct SyncifyLogger;

impl SyncifyLogger {
    pub fn init() {
        spdlog::init_log_crate_proxy().unwrap();

        let sink = Arc::new(StdStreamSink::builder().stdout().build().unwrap());
        let logger = Some(Arc::new(
            Logger::builder().name("main").sink(sink).build().unwrap()
        ));

        log::set_max_level(log::LevelFilter::Trace);
        spdlog::log_crate_proxy().set_logger(logger);
    }
}
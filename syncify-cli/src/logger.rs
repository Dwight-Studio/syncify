use spdlog::formatter::{PatternFormatter, pattern};
use spdlog::sink::{Sink, StdStreamSink};
use spdlog::terminal_style::{Color, Style};
use spdlog::{Level, LevelFilter, Logger};
use std::sync::Arc;

pub struct SyncifyLogger {
    logger: Logger,
}

impl SyncifyLogger {
    pub fn new() -> SyncifyLogger {
        let pattern = pattern!(
            "[{day}-{month}-{year} {time}] [{module_path}/{file_name}:{line}] [{^{level}}] {payload}{eol}"
        );
        let debug_style = Style::builder().color(Color::Cyan).italic().build();
        let info_style = Style::builder().color(Color::Green).build();
        let warn_style = Style::builder().color(Color::Yellow).build();
        let err_style = Style::builder().color(Color::Red).bold().build();

        let mut sink = StdStreamSink::builder().stdout().build().unwrap();
        sink.set_formatter(Box::new(PatternFormatter::new(pattern)));
        sink.set_level_filter(LevelFilter::All);
        sink.set_style(Level::Debug, debug_style);
        sink.set_style(Level::Info, info_style);
        sink.set_style(Level::Warn, warn_style);
        sink.set_style(Level::Error, err_style);

        let logger = Logger::builder()
            .sink(Arc::new(sink))
            .level_filter(LevelFilter::MoreSevereEqual(Level::Debug))
            .build()
            .unwrap();

        spdlog::init_log_crate_proxy().unwrap();

        log::set_max_level(log::LevelFilter::Trace);
        spdlog::log_crate_proxy().set_logger(Some(Arc::new(logger.clone())));

        SyncifyLogger { logger }
    }

    pub fn set_max_level(&self, level: Level) {
        self.logger
            .set_level_filter(LevelFilter::MoreSevereEqual(level));
        spdlog::log_crate_proxy().set_logger(Some(Arc::new(self.logger.clone())));
    }
}

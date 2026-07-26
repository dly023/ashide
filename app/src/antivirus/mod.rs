//! Module containing utilities to query the currently running antivirus / EDR software on the
//! user's machine.

#[cfg(windows)]
mod windows;

use warpui::{Entity, ModelContext, SingletonEntity};

/// Singleton model that reports the currently running antivirus software.
#[derive(Debug, Clone)]
#[cfg(windows)]
pub struct AntivirusInfo(Option<String>);

#[derive(Debug, Clone)]
#[cfg(not(windows))]
pub struct AntivirusInfo;

impl AntivirusInfo {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        cfg_if::cfg_if! {
            if #[cfg(windows)] {
                _ctx.spawn(async move { Self::scan().await }, Self::on_scan_complete);
                Self(None)
            } else {
                Self
            }
        }
    }

    #[cfg(windows)]
    pub fn get(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// Returns the currently running antivirus software for crash tags.
    ///
    /// If called before the antivirus is computed (i.e. before
    /// [`AntivirusInfoEvent::ScannedComplete`] is emitted), this function
    /// returns `None`. It always returns `None` on non-Windows platforms.
    #[allow(dead_code)]
    pub fn crash_report_name(&self) -> Option<&str> {
        #[cfg(windows)]
        return self.get();

        #[cfg(not(windows))]
        return None;
    }
}

pub enum AntivirusInfoEvent {
    #[allow(dead_code)]
    ScannedComplete,
}

impl Entity for AntivirusInfo {
    type Event = AntivirusInfoEvent;
}

impl SingletonEntity for AntivirusInfo {}

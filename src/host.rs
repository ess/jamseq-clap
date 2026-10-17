use clack_extensions::note_name::{HostNoteName, HostNoteNameImpl};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
};
use clack_extensions::timer::{HostTimer, HostTimerImpl, TimerId};
use clack_host::host::HostError;
use clack_host::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct JamSeqClapShared {
    pub rescan_needed: Arc<AtomicBool>,
}

impl<'a> SharedHandler<'a> for JamSeqClapShared {
    fn request_restart(&self) {}
    fn request_process(&self) {}
    fn request_callback(&self) {}
}

impl HostParamsImplShared for JamSeqClapShared {
    fn request_flush(&self) {}
}

pub struct JamSeqClapMainThread {
    pub timers: Arc<Mutex<Vec<TimerId>>>,
    pub rescan_needed: Arc<AtomicBool>,
    pub note_names_changed: Arc<AtomicBool>,
}

impl<'a> MainThreadHandler<'a> for JamSeqClapMainThread {}

impl HostParamsImplMainThread for JamSeqClapMainThread {
    fn rescan(&self, _flags: ParamRescanFlags) {
        self.rescan_needed.store(true, Ordering::Relaxed);
    }

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostTimerImpl for JamSeqClapMainThread {
    fn register_timer(&self, _period_ms: u32) -> Result<TimerId, HostError> {
        let mut timers = self.timers.lock().unwrap();
        let id = TimerId((timers.len() as u32) + 1);
        timers.push(id);
        Ok(id)
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), HostError> {
        self.timers.lock().unwrap().retain(|&id| id != timer_id);
        Ok(())
    }
}

impl HostNoteNameImpl for JamSeqClapMainThread {
    fn changed(&self) {
        self.note_names_changed.store(true, Ordering::Relaxed);
    }
}

pub struct JamSeqClapHost;

impl HostHandlers for JamSeqClapHost {
    type Shared<'a> = JamSeqClapShared;
    type MainThread<'a> = JamSeqClapMainThread;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<'_, Self>, _shared: &JamSeqClapShared) {
        builder.register::<HostTimer>();
        builder.register::<HostParams>();
        builder.register::<HostNoteName>();
    }
}

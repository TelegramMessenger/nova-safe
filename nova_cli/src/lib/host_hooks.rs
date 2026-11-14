// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The [`HostHooks`] implementation for the main thread.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    fmt::Debug,
    path::PathBuf,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use nova_vm::{
    ecmascript::{
        Agent, ExceptionType, GraphLoadingStateRecord, HostDefined, HostHooks, Job, ModuleRequest,
        Referrer, SharedDataBlock, String as JsString, finish_loading_imported_module,
        parse_module,
    },
    engine::{Bindable, Global, NoGcScope},
};

pub enum HostToChildMessage {
    Broadcast(SharedDataBlock),
}

pub enum ChildToHostMessage {
    Joined,
    Report(String),
}

pub struct CliHostHooks {
    promise_job_queue: RefCell<VecDeque<Job>>,
    macrotask_queue: RefCell<Vec<(Option<Instant>, Job)>>,
    pub(crate) receiver: mpsc::Receiver<ChildToHostMessage>,
    pub(crate) own_sender: mpsc::SyncSender<ChildToHostMessage>,
    pub(crate) child_senders: RefCell<Vec<mpsc::SyncSender<HostToChildMessage>>>,
    interrupt_at: Cell<Option<Instant>>,
}

// RefCell doesn't implement Debug
impl Debug for CliHostHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliHostHooks")
            //.field("promise_job_queue", &*self.promise_job_queue.borrow())
            .finish()
    }
}

impl Default for CliHostHooks {
    fn default() -> Self {
        Self::new()
    }
}

impl CliHostHooks {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(10);
        Self {
            promise_job_queue: Default::default(),
            macrotask_queue: Default::default(),
            receiver,
            own_sender: sender,
            child_senders: Default::default(),
            interrupt_at: Cell::new(None),
        }
    }

    /// Interrupts the running script once `after` has passed.
    pub fn set_interrupt_after(&self, after: Duration) {
        self.interrupt_at.set(Some(Instant::now() + after));
    }

    pub fn add_child(&self, child_sender: mpsc::SyncSender<HostToChildMessage>) {
        self.child_senders.borrow_mut().push(child_sender);
    }

    pub fn has_promise_jobs(&self) -> bool {
        !self.promise_job_queue.borrow().is_empty()
    }

    pub fn pop_promise_job(&self) -> Option<Job> {
        self.promise_job_queue.borrow_mut().pop_front()
    }

    pub fn has_macrotasks(&self) -> bool {
        !self.macrotask_queue.borrow().is_empty()
    }

    pub fn pop_macrotask(&self) -> Option<Job> {
        let mut off_thread_job_queue = self.macrotask_queue.borrow_mut();
        let mut counter = 0u8;
        while !off_thread_job_queue.is_empty() {
            counter = counter.wrapping_add(1);
            let now = Instant::now();
            for (i, (deadline, job)) in off_thread_job_queue.iter().enumerate() {
                let deadline_reached = deadline.is_none_or(|d| now >= d);
                if deadline_reached && job.is_finished() {
                    let (_, job) = off_thread_job_queue.swap_remove(i);
                    return Some(job);
                }
            }
            if counter == 0 {
                thread::sleep(Duration::from_millis(5));
            } else {
                core::hint::spin_loop();
            }
        }
        None
    }
}

/// Seconds east of UTC of this machine's time zone, read once from `date +%z`
/// (std has no time zone API; `date` honours `TZ`). 0 when that fails. One
/// offset for every date: a time zone with daylight saving time gets the
/// offset in effect when the program started.
pub fn machine_time_zone_offset() -> i32 {
    static OFFSET: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        let Ok(output) = std::process::Command::new("date").arg("+%z").output() else {
            return 0;
        };
        let text = std::string::String::from_utf8_lossy(&output.stdout);
        let text = text.trim();
        let (sign, digits) = match text.as_bytes().first() {
            Some(b'-') => (-1, &text[1..]),
            Some(b'+') => (1, &text[1..]),
            _ => return 0,
        };
        if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return 0;
        }
        let hours: i32 = digits[..2].parse().unwrap_or(0);
        let minutes: i32 = digits[2..].parse().unwrap_or(0);
        sign * (hours * 3600 + minutes * 60)
    })
}

impl HostHooks for CliHostHooks {
    fn local_time_zone_offset(&self, _epoch_seconds: i64) -> i32 {
        machine_time_zone_offset()
    }

    fn check_interrupt(&self) -> bool {
        self.interrupt_at
            .get()
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    fn enqueue_generic_job(&self, job: Job) {
        self.macrotask_queue.borrow_mut().push((None, job));
    }

    fn enqueue_promise_job(&self, job: Job) {
        self.promise_job_queue.borrow_mut().push_back(job);
    }

    fn enqueue_timeout_job(&self, timeout_job: Job, milliseconds: u64) {
        let deadline = Instant::now() + Duration::from_millis(milliseconds);
        self.macrotask_queue
            .borrow_mut()
            .push((Some(deadline), timeout_job));
    }

    fn load_imported_module<'gc>(
        &self,
        agent: &mut Agent,
        referrer: Referrer<'gc>,
        module_request: ModuleRequest<'gc>,
        _host_defined: Option<HostDefined>,
        payload: &mut GraphLoadingStateRecord<'gc>,
        gc: NoGcScope<'gc, '_>,
    ) {
        let specifier = module_request.specifier(agent);
        let specifier = specifier.to_string_lossy(agent);
        let get_referrer_path = || {
            referrer
                .host_defined(agent)
                .unwrap()
                .downcast::<PathBuf>()
                .unwrap()
        };
        let specifier_target = crate::module_map::specifier_target(specifier, get_referrer_path);
        let realm = referrer.realm(agent, gc);
        let module_map = realm
            .host_defined(agent)
            .expect("No referrer realm [[HostDefined]] slot")
            .downcast::<super::ModuleMap>()
            .expect("No referrer realm ModuleMap");
        if let Some(module) = module_map.get(agent, &specifier_target, gc) {
            finish_loading_imported_module(
                agent,
                referrer,
                module_request,
                payload,
                Ok(module),
                gc,
            );
            return;
        }
        let file = match std::fs::read_to_string(&specifier_target) {
            Ok(file) => file,
            Err(err) => {
                let result = Err(agent.throw_exception(ExceptionType::Error, err.to_string(), gc));
                finish_loading_imported_module(
                    agent,
                    referrer,
                    module_request,
                    payload,
                    result,
                    gc,
                );
                return;
            }
        };
        let source_text = JsString::from_string(agent, file, gc);
        let result = parse_module(
            agent,
            source_text,
            referrer.realm(agent, gc),
            Some(Rc::new(specifier_target.clone())),
            gc,
        )
        .map(|m| {
            let global_m = Global::new(agent, m.unbind().into());
            module_map.add(specifier_target, global_m);
            m.into()
        })
        .map_err(|err| {
            agent.throw_exception(
                ExceptionType::SyntaxError,
                err.first().unwrap().to_string(),
                gc,
            )
        });
        finish_loading_imported_module(agent, referrer, module_request, payload, result, gc);
    }

    fn get_host_data(&self) -> &dyn std::any::Any {
        self
    }
}

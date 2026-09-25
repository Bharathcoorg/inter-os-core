//! Long-lived Ring-3 service lifecycle registry.
//!
//! The supervisor tracks service ownership and restart policy. Execution remains
//! in userspace and service launches continue to use the package trust pipeline.

use alloc::vec::Vec;
use spin::Mutex;
use crate::task::TaskId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState { Registered, Starting, Running, Failed, Stopped }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartPolicy { Never, OnFailure, Always }

#[derive(Clone, Copy, Debug)]
pub struct ServiceRecord {
    pub name: &'static str,
    pub task_id: Option<TaskId>,
    pub state: ServiceState,
    pub restart_policy: RestartPolicy,
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    pub heartbeat: u64,
}

pub struct ServiceSupervisor {
    records: Vec<ServiceRecord>,
    epoch: u64,
}

impl ServiceSupervisor {
    pub const fn new() -> Self {
        Self { records: Vec::new(), epoch: 0 }
    }

    pub fn register(&mut self, name: &'static str, policy: RestartPolicy) {
        if self.records.iter().any(|r| r.name == name) { return; }
        self.records.push(ServiceRecord {
            name, task_id: None, state: ServiceState::Registered,
            restart_policy: policy, restart_count: 0,
            last_exit_code: None, heartbeat: self.epoch,
        });
    }

    pub fn bind_task(&mut self, name: &'static str, task_id: TaskId) {
        if let Some(r) = self.records.iter_mut().find(|r| r.name == name) {
            r.task_id = Some(task_id);
            r.state = ServiceState::Running;
            r.heartbeat = self.epoch;
        }
    }

    pub fn heartbeat(&mut self, task_id: TaskId) {
        self.epoch = self.epoch.saturating_add(1);
        if let Some(r) = self.records.iter_mut().find(|r| r.task_id == Some(task_id)) {
            r.heartbeat = self.epoch;
            r.state = ServiceState::Running;
        }
    }

    pub fn record_exit(&mut self, task_id: TaskId, code: i32) {
        if let Some(r) = self.records.iter_mut().find(|r| r.task_id == Some(task_id)) {
            r.last_exit_code = Some(code);
            r.task_id = None;
            r.restart_count = r.restart_count.saturating_add(1);
            r.state = if code == 0 && matches!(r.restart_policy, RestartPolicy::Never) {
                ServiceState::Stopped
            } else {
                ServiceState::Failed
            };
        }
    }

    pub fn restart_candidates(&self, out: &mut [&'static str; 8]) -> usize { let mut n=0; for r in &self.records { if r.task_id.is_none() && r.state==ServiceState::Failed && matches!(r.restart_policy,RestartPolicy::Always|RestartPolicy::OnFailure) && n<out.len(){out[n]=r.name;n+=1;} } n }

    pub fn should_restart(&self, name: &str) -> bool {
        self.records.iter().find(|r| r.name == name).map(|r| {
            r.task_id.is_none() &&
            r.state == ServiceState::Failed &&
            matches!(r.restart_policy, RestartPolicy::Always | RestartPolicy::OnFailure)
        }).unwrap_or(false)
    }

    pub fn task_for(&self, name: &str) -> Option<TaskId> {
        self.records.iter().find(|r| r.name == name).and_then(|r| r.task_id)
    }

    pub fn snapshot(&self, out: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct W<'a>{b:&'a mut [u8],n:usize}
        impl<'a> Write for W<'a>{fn write_str(&mut self,s:&str)->core::fmt::Result{let k=s.len().min(self.b.len().saturating_sub(self.n));self.b[self.n..self.n+k].copy_from_slice(&s.as_bytes()[..k]);self.n+=k;Ok(())}}
        let mut w=W{b:out,n:0};
        for r in &self.records {
            let state=match r.state{ServiceState::Registered=>"registered",ServiceState::Starting=>"starting",ServiceState::Running=>"running",ServiceState::Failed=>"failed",ServiceState::Stopped=>"stopped"};
            let id=r.task_id.map(|x|x.0).unwrap_or(0);
            let _=write!(w,"{}|{}|{}|{}|{}\n",r.name,state,id,r.restart_count,r.last_exit_code.unwrap_or(0));
        }
        w.n
    }
}

pub static SERVICE_SUPERVISOR: Mutex<ServiceSupervisor> =
    Mutex::new(ServiceSupervisor::new());

pub fn init() {
    let mut s = SERVICE_SUPERVISOR.lock();
    s.register("compositor", RestartPolicy::Always);
    s.register("launcher", RestartPolicy::Always);
    s.register("package", RestartPolicy::Always);
    s.register("app_settings", RestartPolicy::OnFailure);
    s.register("app_terminal", RestartPolicy::OnFailure);
}

//! Personal alerts: `/notify me` sends someone a direct message when a talkgroup they care about
//! has a call, a signal of theirs is heard, or an emergency is declared. Kept in the data folder
//! (`alerts.json`).

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

const FILE: &str = "alerts.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alert {
    pub user: u64,
    /// Empty: any system.
    pub system: String,
    pub talkgroup: Option<u32>,
    /// A signal's ID (`STATION_1`, or `system:STATION_1`).
    pub signal: String,
    /// Only emergencies (on the talkgroup, when there is one).
    pub emergency: bool,
    /// Attach the recording (talkgroup alerts).
    pub audio: bool,
}

impl Alert {
    pub fn describe(&self) -> String {
        let on = if self.system.is_empty() { String::new() } else { format!(" on {}", self.system) };
        match (&self.talkgroup, self.signal.is_empty(), self.emergency) {
            (_, false, _) => format!("signal {}{on}", crate::thread::signal_name(&self.signal)),
            (Some(tg), _, true) => format!("emergencies on TG {tg}{on}"),
            (Some(tg), _, false) => format!("calls on TG {tg}{on}"),
            (None, _, _) => format!("emergencies{on}"),
        }
    }

    fn system_ok(&self, system: &str) -> bool {
        self.system.is_empty() || self.system == system
    }

    /// It wants this recorded call.
    pub fn takes_call(&self, system: &str, talkgroup: u32, emergency: bool) -> bool {
        self.signal.is_empty() && self.system_ok(system) && self.talkgroup.is_some_and(|t| t == talkgroup) && (!self.emergency || emergency)
    }

    /// It wants this emergency (a call that's just been flagged).
    pub fn takes_emergency(&self, system: &str, talkgroup: u32) -> bool {
        self.signal.is_empty() && self.emergency && self.system_ok(system) && self.talkgroup.is_none_or(|t| t == talkgroup)
    }

    /// It wants this tone (its signals).
    pub fn takes_tone(&self, system: &str, signals: &[String]) -> bool {
        !self.signal.is_empty() && self.system_ok(system) && signals.iter().any(|s| crate::settings::same_signal(s, &self.signal))
    }
}

pub struct Alerts {
    list: Mutex<Vec<Alert>>,
    path: PathBuf,
}

impl Alerts {
    pub fn load(dir: &std::path::Path) -> Alerts {
        let path = dir.join(FILE);
        let list = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Alerts { list: Mutex::new(list), path }
    }

    fn save(&self, l: &[Alert]) -> Result<(), String> {
        let j = serde_json::to_vec_pretty(l).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.new");
        std::fs::write(&tmp, j).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| format!("couldn't save the alerts: {e}"))
    }

    pub fn all(&self) -> Vec<Alert> {
        self.list.lock().unwrap().clone()
    }

    pub fn of(&self, user: u64) -> Vec<Alert> {
        self.all().into_iter().filter(|a| a.user == user).collect()
    }

    pub fn add(&self, a: Alert) -> Result<(), String> {
        let mut l = self.list.lock().unwrap();
        if l.contains(&a) {
            return Err("You have that alert already.".into());
        }
        if l.iter().filter(|x| x.user == a.user).count() >= 25 {
            return Err("That's 25 alerts: remove one first.".into());
        }
        l.push(a);
        self.save(&l)
    }

    /// Remove `user`'s alert described as `what`.
    pub fn remove(&self, user: u64, what: &str) -> Result<Alert, String> {
        let mut l = self.list.lock().unwrap();
        let at = l.iter().position(|a| a.user == user && a.describe().eq_ignore_ascii_case(what.trim())).ok_or("You have no such alert.")?;
        let a = l.remove(at);
        self.save(&l)?;
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alerts_pick_what_they_say() {
        let tg = Alert { user: 1, talkgroup: Some(1001), ..Default::default() };
        assert!(tg.takes_call("clmrn-I", 1001, false) && !tg.takes_call("clmrn-I", 1002, false));
        let em = Alert { user: 1, emergency: true, system: "clmrn-I".into(), ..Default::default() };
        assert!(em.takes_emergency("clmrn-I", 5) && !em.takes_emergency("clmrn-G", 5));
        let sig = Alert { user: 1, signal: "STATION_1".into(), ..Default::default() };
        assert!(sig.takes_tone("clmrn-I", &["clmrn-I:STATION_1".into()]) && !sig.takes_call("clmrn-I", 1001, false));
        assert_eq!(sig.describe(), "signal STATION 1");
    }
}

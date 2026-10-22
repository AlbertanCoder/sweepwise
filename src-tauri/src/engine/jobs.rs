// Background jobs (scan, clean, security check, disk map...): one at a time, with live progress lines.
use serde_json::{json, Map, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct Job {
    pub id: String,
    pub mode: String,
    started: Instant,
    inner: Mutex<Inner>,
}

struct Inner {
    lines: Vec<String>,
    step: String,
    percent: u32,
    done: bool,
    exit_code: Option<i32>,
    run_id: Option<String>,
    extra: Map<String, Value>,
    log: PathBuf,
}

impl Job {
    pub fn new(id: String, mode: &str, log: PathBuf) -> Arc<Job> {
        Arc::new(Job {
            id,
            mode: mode.to_string(),
            started: Instant::now(),
            inner: Mutex::new(Inner {
                lines: Vec::new(),
                step: "Starting...".into(),
                percent: 3,
                done: false,
                exit_code: None,
                run_id: None,
                extra: Map::new(),
                log,
            }),
        })
    }

    pub fn running(&self) -> bool {
        !self.inner.lock().map(|i| i.done).unwrap_or(true)
    }

    /// One progress line (also appended to the job's log file).
    pub fn say(&self, line: impl Into<String>) {
        let line = line.into();
        if let Ok(mut i) = self.inner.lock() {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&i.log) {
                let _ = writeln!(f, "{}", line);
            }
            i.lines.push(line);
            if i.lines.len() > 400 {
                i.lines.drain(0..100);
            }
        }
    }

    /// "Step 2 of 5: ..." moves the progress bar.
    pub fn step(&self, n: u32, total: u32, text: &str) {
        let line = format!("Step {} of {}: {}", n, total, text);
        if let Ok(mut i) = self.inner.lock() {
            i.step = text.to_string();
            i.percent = (((n.saturating_sub(1)) as f64 / total.max(1) as f64) * 100.0).round() as u32 + 5;
        }
        self.say(line);
    }

    pub fn set_run(&self, id: &str) {
        if let Ok(mut i) = self.inner.lock() {
            i.run_id = Some(id.to_string());
        }
    }

    pub fn set_extra(&self, key: &str, v: Value) {
        if let Ok(mut i) = self.inner.lock() {
            i.extra.insert(key.to_string(), v);
        }
    }

    pub fn finish(&self, code: i32) {
        if let Ok(mut i) = self.inner.lock() {
            i.done = true;
            i.exit_code = Some(code);
            i.percent = 100;
            i.step = "Finished".into();
        }
    }

    pub fn state(&self, temp_c: Option<f64>) -> Value {
        let i = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => return json!({ "running": false }),
        };
        let skip = i.lines.len().saturating_sub(14);
        let mut v = json!({
            "running": !i.done,
            "mode": self.mode,
            "id": self.id,
            "step": i.step,
            "percent": i.percent,
            "lines": i.lines[skip..].to_vec(),
            "exit_code": i.exit_code,
            "run_id": i.run_id,
            "seconds": self.started.elapsed().as_secs(),
            "temp_c": temp_c,
        });
        if let Some(obj) = v.as_object_mut() {
            for (k, val) in i.extra.iter() {
                obj.insert(k.clone(), val.clone());
            }
        }
        v
    }

    pub fn lines(&self) -> Vec<String> {
        self.inner.lock().map(|i| i.lines.clone()).unwrap_or_default()
    }
}

/// Runs `work` on its own thread; a panic becomes a readable failure instead of a crash.
pub fn spawn<F>(job: Arc<Job>, work: F)
where
    F: FnOnce(&Job) -> i32 + Send + 'static,
{
    std::thread::spawn(move || {
        let j = job.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || work(&j)));
        match result {
            Ok(code) => job.finish(code),
            Err(e) => {
                let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "unknown problem".into());
                job.say(format!("Something went wrong: {}", msg));
                job.finish(1);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_reports_progress_and_finishes() {
        let dir = std::env::temp_dir().join(format!("sw_jobs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let job = Job::new("j1".into(), "clean", dir.join("j1.log"));
        let j2 = job.clone();
        spawn(job.clone(), move |j| {
            j.step(1, 2, "first");
            j.step(2, 2, "second");
            j.set_extra("preview", json!(true));
            0
        });
        for _ in 0..100 {
            if !j2.running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let s = j2.state(Some(55.0));
        assert_eq!(s["running"], false);
        assert_eq!(s["percent"], 100);
        assert_eq!(s["exit_code"], 0);
        assert_eq!(s["preview"], true);
        assert!(s["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("Step 2 of 2")));
        assert!(std::fs::read_to_string(dir.join("j1.log")).unwrap().contains("Step 1 of 2"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_panic_becomes_a_failed_job() {
        let dir = std::env::temp_dir().join(format!("sw_jobs_p_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let job = Job::new("j2".into(), "quick", dir.join("j2.log"));
        spawn(job.clone(), |_j| panic!("boom"));
        for _ in 0..100 {
            if !job.running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(job.state(None)["exit_code"], 1);
        assert!(job.lines().iter().any(|l| l.contains("boom")));
        let _ = std::fs::remove_dir_all(dir);
    }
}

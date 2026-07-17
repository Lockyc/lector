//! The serve-loop supervisor. lector never renders anything itself — each *live* tab owns a
//! compositor `ServeHandle` (one thread running compositor's serve loop on an ephemeral loopback
//! port, plus its own watcher); a *cold* tab owns nothing. The `live` dot means exactly "this
//! repo's server is up and watching".

use compositor::ServeHandle;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

/// One live site.
struct SiteServer {
    port: u16,
    handle: ServeHandle,
}

/// The registry of live servers, keyed by `TabView::label`. A label absent from the map is a cold
/// tab — the two states are the map's membership, not a flag, so they cannot disagree.
#[derive(Default)]
pub struct Servers {
    live: Mutex<HashMap<String, SiteServer>>,
}

impl Servers {
    pub fn new() -> Servers {
        Servers::default()
    }

    /// Start `dir`'s server if this tab is cold, and return its port. Idempotent: an already-live
    /// tab returns its existing port rather than spawning a second server for the same repo.
    ///
    /// On failure (missing dir, bind failure) nothing is registered — the tab stays cold and the
    /// caller sends the message to the chrome's `setError`. That is the error channel; per-repo
    /// build health is explicitly out of scope.
    pub fn start(&self, label: &str, dir: &Path) -> Result<u16, String> {
        let mut live = self.live.lock().expect("servers lock");
        if let Some(s) = live.get(label) {
            return Ok(s.port);
        }
        let handle = compositor::serve_handle(dir).map_err(|e| format!("{e:#}"))?;
        let port = handle.port;
        live.insert(label.to_string(), SiteServer { port, handle });
        Ok(port)
    }

    pub fn port(&self, label: &str) -> Option<u16> {
        self.live
            .lock()
            .expect("servers lock")
            .get(label)
            .map(|s| s.port)
    }

    pub fn is_live(&self, label: &str) -> bool {
        self.live.lock().expect("servers lock").contains_key(label)
    }

    /// Stop one tab's server, joining its threads.
    pub fn stop(&self, label: &str) {
        let removed = self.live.lock().expect("servers lock").remove(label);
        // Shut down outside the lock: shutdown() joins two threads, and holding the registry lock
        // across a join would block every other tab's start/select for its duration.
        if let Some(s) = removed {
            s.handle.shutdown();
        }
    }

    /// Stop every server whose label is not in `keep`. Called on config hot-reload: a removed tab
    /// must have its server shut down and threads joined, or every config edit leaks a watcher and
    /// a port.
    pub fn retain(&self, keep: &HashSet<String>) {
        let dropped: Vec<SiteServer> = {
            let mut live = self.live.lock().expect("servers lock");
            let gone: Vec<String> = live
                .keys()
                .filter(|l| !keep.contains(*l))
                .cloned()
                .collect();
            gone.iter().filter_map(|l| live.remove(l)).collect()
        };
        for s in dropped {
            s.handle.shutdown();
        }
    }

    /// Stop everything, joining all threads. Called on quit.
    pub fn shutdown_all(&self) {
        let all: Vec<SiteServer> = self
            .live
            .lock()
            .expect("servers lock")
            .drain()
            .map(|(_, s)| s)
            .collect();
        for s in all {
            s.handle.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway doc repo with one page.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lector-srv-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/index.md"), "# Scratch\n").unwrap();
        dir
    }

    fn get(port: u16, path: &str) -> String {
        use std::io::{Read, Write};
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = std::net::TcpStream::connect(addr).unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        resp
    }

    #[test]
    fn start_serves_the_repo_and_reports_live() {
        let dir = scratch("start");
        let s = Servers::new();
        assert!(!s.is_live("t1"), "a cold tab owns nothing");
        assert_eq!(s.port("t1"), None);

        let port = s.start("t1", &dir).expect("server starts");
        assert!(port > 0);
        assert!(s.is_live("t1"));
        assert_eq!(s.port("t1"), Some(port));
        assert!(get(port, "/").contains("200 OK"));

        s.shutdown_all();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn start_is_idempotent_and_reuses_the_port() {
        // Selecting an already-live tab must not spawn a second server for the same repo.
        let dir = scratch("idem");
        let s = Servers::new();
        let a = s.start("t1", &dir).unwrap();
        let b = s.start("t1", &dir).unwrap();
        assert_eq!(a, b, "a live tab must reuse its server, not start another");
        s.shutdown_all();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn start_on_a_missing_dir_errors_and_stays_cold() {
        // The spec's lifecycle rule: start failure → the message goes to the chrome's setError and
        // the tab stays cold. It must not half-register.
        let s = Servers::new();
        let err = s
            .start("t1", std::path::Path::new("/definitely/not/here"))
            .unwrap_err();
        assert!(!err.is_empty());
        assert!(!s.is_live("t1"), "a failed start must leave the tab cold");
        assert_eq!(s.port("t1"), None);
    }

    #[test]
    fn stop_frees_the_tab() {
        let dir = scratch("stop");
        let s = Servers::new();
        s.start("t1", &dir).unwrap();
        s.stop("t1");
        assert!(!s.is_live("t1"));
        assert_eq!(s.port("t1"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn retain_stops_servers_for_removed_tabs() {
        // The config-hot-reload leak: a removed tab must have its server shut down, or every edit
        // leaks a watcher and a port.
        let a = scratch("retain-a");
        let b = scratch("retain-b");
        let s = Servers::new();
        s.start("keep", &a).unwrap();
        s.start("drop", &b).unwrap();

        let keep: std::collections::HashSet<String> = ["keep".to_string()].into_iter().collect();
        s.retain(&keep);

        assert!(s.is_live("keep"));
        assert!(!s.is_live("drop"), "a removed tab's server must be stopped");
        s.shutdown_all();
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn shutdown_all_stops_every_server() {
        let a = scratch("all-a");
        let b = scratch("all-b");
        let s = Servers::new();
        s.start("t1", &a).unwrap();
        s.start("t2", &b).unwrap();
        s.shutdown_all();
        assert!(!s.is_live("t1"));
        assert!(!s.is_live("t2"));
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn repeated_start_stop_cycles_do_not_leak_ports() {
        // The config-hot-reload path start/stops on every edit. A leaked ServeHandle would keep its
        // thread and watcher alive; 20 cycles would leave 20 of each. Ports are ephemeral so a leak
        // won't EADDRINUSE — it shows up as an unbounded thread count, which is why this asserts on
        // the registry emptying rather than on a bind.
        let dir = scratch("cycle");
        let s = Servers::new();
        for _ in 0..20 {
            s.start("t1", &dir).unwrap();
            assert!(s.is_live("t1"));
            s.stop("t1");
            assert!(!s.is_live("t1"));
        }
        assert!(s.live.lock().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}

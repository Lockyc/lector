//! The serve-loop supervisor. lector never renders anything itself — each *live* tab owns a
//! compositor `ServeHandle` (one thread running compositor's serve loop on an ephemeral loopback
//! port, plus its own watcher); a *cold* tab owns nothing. The `live` dot means exactly "this
//! repo's server is up and watching".

use compositor::ServeHandle;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

/// One live site. `handle` is `None` only for a registry entry that a panicked serve thread has
/// left behind (see [`Servers::register_unserved_for_test`]) — a real `start()` always inserts
/// `Some`. Modeling it as optional here is what makes the dead-registration state constructible
/// for `reap`'s test without bending the production type.
struct SiteServer {
    port: u16,
    handle: Option<ServeHandle>,
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
    ///
    /// `compositor::serve_handle` walks and renders the whole docs tree before returning
    /// (measured 150-450ms on real repos), so it must run outside the registry lock — holding
    /// the lock across it would stall every other tab's `is_live`/`port`/`start`/`stop` behind
    /// one cold start, exactly the freeze the module avoids for `shutdown()`'s thread joins.
    /// That leaves a window where two concurrent `start()` calls for the same label can both
    /// build, so the insert is double-checked: after the build, re-take the lock, re-check
    /// membership, and if another thread already won, discard this thread's own handle (shut
    /// down outside the lock, per the same rule) and return the winner's port. Accepted cost: in
    /// that rare race, one build is wasted and one transient bind happens — strictly better than
    /// stalling every tab for up to ~450ms on every cold start.
    pub fn start(&self, label: &str, dir: &Path) -> Result<u16, String> {
        if let Some(s) = self.live.lock().expect("servers lock").get(label) {
            return Ok(s.port);
        }
        let handle = compositor::serve_handle(dir).map_err(|e| format!("{e:#}"))?;
        let port = handle.port;

        let loser = {
            let mut live = self.live.lock().expect("servers lock");
            if let Some(s) = live.get(label) {
                // Another thread won the race while we were building. Keep the winner, discard
                // ours — but not under the lock (see the doc comment above).
                Some((
                    s.port,
                    SiteServer {
                        port,
                        handle: Some(handle),
                    },
                ))
            } else {
                live.insert(
                    label.to_string(),
                    SiteServer {
                        port,
                        handle: Some(handle),
                    },
                );
                None
            }
        };
        if let Some((winner_port, ours)) = loser {
            if let Some(h) = ours.handle {
                h.shutdown();
            }
            return Ok(winner_port);
        }
        Ok(port)
    }

    pub fn port(&self, label: &str) -> Option<u16> {
        self.live
            .lock()
            .expect("servers lock")
            .get(label)
            .map(|s| s.port)
    }

    /// Raw registry membership — "did `start()` register this label", with no liveness probe.
    /// `is_alive` (below) is what `commands::tab_dtos` actually uses for the sidebar's `live` dot
    /// (it probes the port too, which is the whole point of dead-thread detection); this one stays
    /// a public primitive in its own right, exercised directly by this module's own tests (e.g.
    /// `reap_drops_a_registered_tab_whose_server_stopped_answering` asserts the registry believes a
    /// dead tab is live via this exact method, before `reap` corrects it).
    #[allow(dead_code)] // no non-test caller yet — a deliberately-kept lower-level primitive
    pub fn is_live(&self, label: &str) -> bool {
        self.live.lock().expect("servers lock").contains_key(label)
    }

    /// Stop one tab's server, joining its threads. A no-op on the handle for an entry `reap` (or
    /// the test constructor) already left with no `ServeHandle` — there is nothing to shut down.
    pub fn stop(&self, label: &str) {
        let removed = self.live.lock().expect("servers lock").remove(label);
        // Shut down outside the lock: shutdown() joins two threads, and holding the registry lock
        // across a join would block every other tab's start/select for its duration.
        if let Some(s) = removed {
            if let Some(h) = s.handle {
                h.shutdown();
            }
        }
    }

    /// Stop every server whose label is not in `keep`. Called by `reload::reconcile` on both
    /// launch and config hot-reload: a removed tab must have its server shut down and threads
    /// joined, or every config edit leaks a watcher and a port.
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
            if let Some(h) = s.handle {
                h.shutdown();
            }
        }
    }

    /// Stop everything, joining all threads. Called on quit (`RunEvent::Exit`, in `lib.rs`).
    pub fn shutdown_all(&self) {
        let all: Vec<SiteServer> = self
            .live
            .lock()
            .expect("servers lock")
            .drain()
            .map(|(_, s)| s)
            .collect();
        for s in all {
            if let Some(h) = s.handle {
                h.shutdown();
            }
        }
    }
}

/// Does anything answer on this loopback port? The liveness primitive, split out as a free
/// function so it is directly testable against a port nobody is serving — a dead serve thread is
/// indistinguishable, from the outside, from a port that was never bound.
///
/// A closed loopback port refuses immediately (RST), so the timeout only bites on a black-holed
/// port, which does not happen on loopback. The refresh path can afford this per tab.
fn port_answers(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(50),
    )
    .is_ok()
}

impl Servers {
    /// True iff this tab's server is registered AND still answering. A panic in the serve loop
    /// kills that thread silently (compositor's FOLLOWUPS records this: the loop is one
    /// background thread among N here, so a panic doesn't abort the process — it just stops that
    /// site). The registry alone would still report `live`, so probe the port before believing it.
    pub fn is_alive(&self, label: &str) -> bool {
        self.port(label).is_some_and(port_answers)
    }

    /// Drop any tab whose server has died, so the chrome's `live` dot goes hollow and re-selecting
    /// retries. Called on each chrome refresh (`get_tabs`).
    pub fn reap(&self) {
        let registered: Vec<String> = {
            let live = self.live.lock().expect("servers lock");
            live.keys().cloned().collect()
        };
        for label in registered.into_iter().filter(|l| !self.is_alive(l)) {
            self.stop(&label);
        }
    }

    /// Register a port with no server behind it — the state a panicked serve loop leaves (the
    /// registry still lists the label, but nothing answers on its port). Test-only: production
    /// code always inserts through `start()`, which never registers a `None` handle.
    #[cfg(test)]
    fn register_unserved_for_test(&self, label: &str, port: u16) {
        self.live
            .lock()
            .expect("servers lock")
            .insert(label.to_string(), SiteServer { port, handle: None });
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

    /// A doc repo with `n` pages, each carrying a few fenced code blocks. Real build cost is
    /// dominated by markdown/syntax-highlight rendering per page, so this is what makes a
    /// scratch repo's build take a non-trivial, roughly-linear-in-`n` amount of wall time
    /// instead of the sub-millisecond `scratch()` above. Calibrated on this machine (debug
    /// build): 5 pages ~184ms, 8 pages ~292ms — see `start_does_not_stall_other_calls_during_a_cold_build`
    /// for why that matters.
    fn scratch_big(name: &str, n: usize) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lector-srv-big-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        for i in 1..=n {
            let content = format!(
                "# Page {i}\n\nSome prose about page {i}, enough that the renderer has real \
                 parsing work to do.\n\n\
                 ```rust\nfn f() -> u32 {{ {i} }}\n```\n\n\
                 ```python\ndef f():\n    return {i}\n```\n\n\
                 ```bash\necho {i}\n```\n\n\
                 - a\n- b\n- c\n"
            );
            std::fs::write(dir.join(format!("docs/page{i}.md")), content).unwrap();
        }
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
    fn concurrent_start_on_the_same_label_registers_exactly_one_server() {
        // Exercises the double-checked insert directly: many threads racing start() for the
        // same label must still end up with exactly one registered server — idempotency under
        // real concurrency, not just in sequence — and no loser handle left sitting in the map.
        // Unlike the stall test above this makes no timing assumption, so it cannot flake on a
        // slow machine; it only asserts the final state is correct regardless of how the
        // threads happened to interleave.
        let dir = scratch("race");
        let s = std::sync::Arc::new(Servers::new());

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s = std::sync::Arc::clone(&s);
                let dir = dir.clone();
                std::thread::spawn(move || s.start("t1", &dir).unwrap())
            })
            .collect();
        let ports: Vec<u16> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let first = ports[0];
        assert!(
            ports.iter().all(|&p| p == first),
            "every racing start() must agree on the one winning port: {ports:?}"
        );
        assert_eq!(
            s.live.lock().unwrap().len(),
            1,
            "a race must not register more than one server for the same label"
        );
        assert_eq!(s.port("t1"), Some(first));

        s.shutdown_all();
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
    fn start_does_not_stall_other_calls_during_a_cold_build() {
        // Regression test for the fix: `start()` must build *outside* the registry lock, so
        // another tab's `is_live`/`port` never blocks behind one tab's cold start. Before the
        // fix, `start()` held the lock across `compositor::serve_handle`, which walks and
        // renders the whole docs tree (measured 161-454ms on real doc repos — see servers.rs's
        // `start` doc comment). `scratch_big` reproduces that cost synthetically (calibrated
        // ~292ms for 8 pages on this machine, in the same ballpark as the real measurements) so
        // this test doesn't depend on checking out an unrelated sibling repo to get a slow build.
        //
        // The bound below is a *ratio* of probe latency to the build's own measured duration in
        // this same run — not an absolute millisecond figure. An absolute bound (e.g. "a probe
        // must take <50ms") is what shipped here originally, and it flaked under machine load:
        // this box routinely sits at load average 90-260, and at that load even a perfectly
        // healthy, uncontended probe can be descheduled past any fixed small bound — the test
        // was measuring the machine's scheduler, not `start()`'s locking. Load inflates both the
        // build's wall-clock duration and an occasionally-descheduled probe by roughly the same
        // factor, so their *ratio* stays stable across load while either one's absolute value
        // does not. Do not "fix" a flake here by reverting to an absolute bound — that
        // reintroduces exactly the failure this rewrite removes.
        let big = scratch_big("stall", 8);
        let other = scratch("stall-other");
        let s = std::sync::Arc::new(Servers::new());

        let builder = {
            let s = std::sync::Arc::clone(&s);
            let dir = big.clone();
            std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                let result = s.start("big", &dir);
                (result, t0.elapsed())
            })
        };
        // Let the builder thread get past its fast-path check and into the build itself before
        // we start probing, so we're not racing it to the very first lock acquisition.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut probes = 0u32;
        let mut max_probe = std::time::Duration::ZERO;
        while !builder.is_finished() {
            let t0 = std::time::Instant::now();
            assert!(!s.is_live("other"), "unrelated label must not be live");
            assert_eq!(s.port("other"), None, "unrelated label must have no port");
            let elapsed = t0.elapsed();
            if elapsed > max_probe {
                max_probe = elapsed;
            }
            probes += 1;
        }
        assert!(
            probes > 0,
            "the builder finished before any probe ran \u{2014} scratch_big no longer builds \
             slowly enough to exercise the race; enlarge its page count"
        );

        let (result, build_elapsed) = builder.join().unwrap();
        let port = result.expect("build succeeds");
        assert!(s.is_live("big"));
        assert_eq!(s.port("big"), Some(port));

        // Held-lock bug: a probe blocks until the build releases the lock, so its latency is on
        // the order of the build's *whole* duration — max_probe / build_elapsed lands close to
        // 1.0. Correct (unlocked) code: a probe only touches an uncontended mutex and a HashMap
        // lookup, microsecond-scale work, so the ratio sits near zero even under heavy
        // scheduling noise. 1/5 sits far below the buggy value and comfortably above the noise
        // floor observed in practice (a few percent, from occasional scheduler preemption of the
        // probing thread itself under this machine's load) — pick it and leave it; do not inch
        // it back toward 1 to chase a flake.
        let ratio = max_probe.as_secs_f64() / build_elapsed.as_secs_f64();
        assert!(
            ratio < 0.2,
            "worst probe ({max_probe:?}) was {ratio:.3}x the build's own duration \
             ({build_elapsed:?}) \u{2014} start() looks like it is holding the registry lock \
             across the build"
        );

        s.shutdown_all();
        std::fs::remove_dir_all(&big).ok();
        std::fs::remove_dir_all(&other).ok();
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
    fn port_answers_only_while_something_serves() {
        // The liveness primitive. A port nobody serves is exactly what a died-in-the-night serve
        // thread leaves behind — indistinguishable from outside, which is why this is the probe.
        let dir = scratch("answers");
        let s = Servers::new();
        let port = s.start("t1", &dir).unwrap();
        assert!(port_answers(port), "a live server must answer");

        s.stop("t1");
        // The listener frees on tiny_http's schedule (its accept thread is signalled, not
        // joined), so poll rather than assert once — the same asynchrony compositor's own test
        // hit.
        let stopped = (0..100).any(|_| {
            if !port_answers(port) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            false
        });
        assert!(stopped, "a stopped server must stop answering");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn is_alive_is_false_for_an_unregistered_tab() {
        let s = Servers::new();
        assert!(!s.is_alive("never-started"));
    }

    #[test]
    fn reap_drops_a_registered_tab_whose_server_stopped_answering() {
        // The spec's requirement, and the compositor follow-up it graduates: a serve thread that
        // dies must drop the live dot rather than leave the tab looking live. Registering a port
        // that nothing serves reproduces exactly the state a panicked serve loop leaves — the
        // registry says live, the port says otherwise.
        let s = Servers::new();
        let dead_port = {
            // Bind and immediately drop, so the port is real but unserved.
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        s.register_unserved_for_test("ghost", dead_port);
        assert!(s.is_live("ghost"), "the registry believes it is live");
        assert!(
            !s.is_alive("ghost"),
            "but nothing answers, so it is not alive"
        );

        s.reap();
        assert!(
            !s.is_live("ghost"),
            "reap must drop a tab whose server died"
        );
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

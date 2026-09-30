use std::path::PathBuf;

fn open(path: &str) -> gix::Repository {
    let opts = gix::open::Options::isolated().config_overrides([
        "gitoxide.objects.allocLimit=64m",
        "core.commitGraph=false",
        "index.threads=1",
    ]);
    gix::open_opts(PathBuf::from(path), opts).expect("open")
}

struct D {
    trees: u64,
    nontrees: u64,
}

impl gix::traverse::tree::Visit for D {
    fn pop_back_tracked_path_and_set_current(&mut self) {}
    fn pop_front_tracked_path_and_set_current(&mut self) {}
    fn push_back_tracked_path_component(&mut self, _c: &gix::bstr::BStr) {}
    fn push_path_component(&mut self, _c: &gix::bstr::BStr) {}
    fn pop_path_component(&mut self) {}
    fn visit_tree(&mut self, _e: &gix::objs::tree::EntryRef<'_>) -> std::ops::ControlFlow<(), bool> {
        self.trees += 1;
        std::ops::ControlFlow::Continue(true)
    }
    fn visit_nontree(&mut self, _e: &gix::objs::tree::EntryRef<'_>) -> std::ops::ControlFlow<(), bool> {
        self.nontrees += 1;
        eprintln!("deepest point reached, trees so far={}", self.trees);
        std::thread::sleep(std::time::Duration::from_millis(2500));
        std::ops::ControlFlow::Continue(true)
    }
}

fn main_id(git_dir: &str) -> gix::ObjectId {
    let hex = std::fs::read_to_string(format!("{git_dir}/refs/heads/main")).expect("ref");
    gix::ObjectId::from_hex(hex.trim().as_bytes()).expect("hex")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args[1].as_str();
    let secs: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    std::thread::spawn(move || {
        for tick in 1..=secs {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let out = std::process::Command::new("ps")
                .args(["-o", "rss=", "-p", &std::process::id().to_string()])
                .output()
                .expect("ps");
            eprintln!("t={tick}s rss_kib={}", String::from_utf8_lossy(&out.stdout).trim());
        }
        eprintln!("WATCHDOG: still running after {secs}s, exiting");
        std::process::exit(3);
    });
    let repo = open(&args[2]);
    match mode {
        "shallow" | "walk" => {
            let id = main_id(&args[2]);
            eprintln!("walking from {id}");
            let mut n = 0u64;
            for info in repo.rev_walk([id]).all().expect("walk platform") {
                match info {
                    Ok(_) => n += 1,
                    Err(e) => {
                        eprintln!("walk error: {e}");
                        break;
                    }
                }
            }
            eprintln!("walked {n} commits");
        }
        "tree" => {
            let hex = std::fs::read_to_string(format!("{}/refs/heads/tree", &args[2])).expect("ref");
            let id = gix::ObjectId::from_hex(hex.trim().as_bytes()).expect("hex");
            let mut d = D { trees: 0, nontrees: 0 };
            let r = gix::traverse::tree::depthfirst(
                id,
                gix::traverse::tree::depthfirst::State::default(),
                &repo.objects,
                &mut d,
            );
            eprintln!("depthfirst done: {:?} trees={} nontrees={}", r.is_ok(), d.trees, d.nontrees);
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        "headid" => {
            let id = repo.head_id().expect("head id");
            eprintln!("head_id ok {id}");
        }
        "indexpath" => {
            eprintln!("index_path = {}", repo.index_path().display());
        }
        "replace" => {
            eprintln!("opened; is_shallow={}", repo.is_shallow());
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
        "header" => {
            let id = main_id(&args[2]);
            let r = repo.find_header(id);
            eprintln!("find_header: {:?}", r.map(|h| h.size()));
        }
        _ => panic!("mode"),
    }
}

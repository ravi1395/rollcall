use std::path::PathBuf;

fn overrides(git_dir: &str, with_path_overrides: bool) -> Vec<String> {
    let mut v = vec![
        "gitoxide.objects.allocLimit=64m".to_string(),
        "core.commitGraph=false".to_string(),
        "core.bigFileThreshold=1m".to_string(),
        "index.threads=1".to_string(),
    ];
    if with_path_overrides {
        v.push("gitoxide.core.shallowFile=shallow".into());
        v.push(format!("gitoxide.core.indexFile={git_dir}/index"));
        v.push("gitoxide.objects.replaceRefBase=refs/replace/".into());
    }
    v
}

fn open(git_dir: &str, with_path_overrides: bool, alloc: Option<&str>) -> gix::Repository {
    let mut o = overrides(git_dir, with_path_overrides);
    if let Some(a) = alloc {
        o[0] = format!("gitoxide.objects.allocLimit={a}");
    }
    let opts = gix::open::Options::isolated().open_path_as_is(true).config_overrides(o);
    gix::open_opts(PathBuf::from(git_dir), opts).expect("open")
}

fn strip(repo: &mut gix::Repository) {
    let mut snap = repo.config_snapshot_mut();
    let ids: Vec<_> = snap
        .sections_and_ids()
        .filter(|(s, _)| {
            let n = s.header().name().to_string();
            ["filter", "diff", "merge"].iter().any(|x| n.eq_ignore_ascii_case(x))
        })
        .map(|(_, id)| id)
        .collect();
    for id in ids {
        snap.remove_section_by_id(id);
    }
    snap.set_raw_value("core.excludesFile", "/dev/null").expect("set");
    snap.commit().expect("commit");
}

fn rss_kib() -> String {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mode = a[1].as_str();
    let git_dir = a[2].as_str();
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(6));
        eprintln!("WATCHDOG fired after 6s, rss_kib={}", rss_kib());
        std::process::exit(3);
    });
    match mode {
        // paths <git_dir> <with_overrides 0|1> <strip 0|1>
        "paths" => {
            let with = a[3] == "1";
            let mut repo = open(git_dir, with, None);
            println!("after open: rss_kib={}", rss_kib());
            if a[4] == "1" {
                strip(&mut repo);
            }
            println!("shallow_file={}", repo.shallow_file().display());
            println!("index_path={}", repo.index_path().display());
            let snap = repo.config_snapshot();
            let f = |k: &str| {
                snap.plumbing()
                    .string_filter(k, &mut gix::config::section::is_trusted)
                    .map(|v| v.to_string())
            };
            println!("cfg gitoxide.core.shallowFile={:?}", f("gitoxide.core.shallowFile"));
            println!("cfg gitoxide.core.indexFile={:?}", f("gitoxide.core.indexFile"));
            println!("cfg gitoxide.objects.replaceRefBase={:?}", f("gitoxide.objects.replaceRefBase"));
            println!("cfg core.bigFileThreshold(integer)={:?}", snap.integer("core.bigFileThreshold"));
            println!("cfg core.commitGraph={:?}", snap.boolean("core.commitGraph"));
            println!("done rss_kib={}", rss_kib());
        }
        // walk <git_dir> <with>: head_id + rev_walk
        "walk" => {
            let with = a[3] == "1";
            let mut repo = open(git_dir, with, None);
            strip(&mut repo);
            let id = repo.head_id().expect("head").detach();
            let n = repo.rev_walk([id]).use_commit_graph(false).all().expect("walk").count();
            println!("walked {n} commits, rss_kib={}", rss_kib());
        }
        // refresh <git_dir>
        "refresh" => {
            for never in [false, true] {
                let mut repo = open(git_dir, true, None);
                if never {
                    repo.objects.refresh_never();
                }
                let before = repo.objects.store_ref().metrics().num_refreshes;
                let head = repo.head_id().expect("head").detach();
                let hit = repo.try_find_header(head).unwrap().is_some();
                let after_first = repo.objects.store_ref().metrics().num_refreshes;
                for i in 0..5u8 {
                    let mut raw = [0u8; 20];
                    raw[0] = 0xaa;
                    raw[1] = i;
                    let id = gix::ObjectId::from_bytes_or_panic(&raw);
                    assert!(repo.try_find_header(id).unwrap().is_none());
                }
                let after_misses = repo.objects.store_ref().metrics().num_refreshes;
                let mut clone = repo.objects.clone().into_arc().expect("into_arc");
                println!(
                    "refresh_never={never}: present-hit={hit} refreshes before={before} after_first_lookup={after_first} after_5_misses={after_misses}; into_arc mode is Never: {}",
                    matches!(clone.refresh_mode(), gix::odb::store::RefreshMode::Never)
                );
            }
        }
        // late <git_dir>: objects written after open, loose vs packed, per refresh mode
        "late" => {
            use std::io::Write;
            let run = |args: &[&str], input: Option<&str>| -> String {
                let mut c = std::process::Command::new("git");
                c.arg("--git-dir").arg(git_dir).args(args);
                c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
                let mut ch = c.spawn().unwrap();
                ch.stdin.take().unwrap().write_all(input.unwrap_or("").as_bytes()).unwrap();
                let out = ch.wait_with_output().unwrap();
                String::from_utf8_lossy(&out.stdout).trim().to_string()
            };
            for never in [false, true] {
                let mut repo = open(git_dir, true, None);
                if never {
                    repo.objects.refresh_never();
                }
                let head = repo.head_id().expect("head").detach();
                assert!(repo.try_find_header(head).unwrap().is_some());
                let loose = run(&["hash-object", "-w", "--stdin"], Some(&format!("loose-{never}")));
                let packed = run(&["hash-object", "-w", "--stdin"], Some(&format!("packed-{never}")));
                run(&["pack-objects", &format!("{git_dir}/objects/pack/late-{never}")], Some(&format!("{packed}\n")));
                run(&["prune-packed"], None);
                let f = |h: &str| repo.try_find_header(gix::ObjectId::from_hex(h.as_bytes()).unwrap()).unwrap().is_some();
                println!("refresh_never={never}: loose-after-open found={} packed-after-open found={}", f(&loose), f(&packed));
            }
        }
        // header <git_dir> <tree_hex> <alloc>
        "header" => {
            let id = gix::ObjectId::from_hex(a[3].as_bytes()).unwrap();
            let repo = open(git_dir, true, Some(&a[4]));
            let h = repo.find_header(id).expect("header");
            let variant = match h {
                gix::odb::find::Header::Loose { .. } => "loose",
                gix::odb::find::Header::Packed(_) => "packed",
            };
            println!("find_header: kind={:?} size={} variant={variant}", h.kind(), h.size());
            println!("rss_kib after header={}", rss_kib());
            match repo.find_object(id) {
                Ok(o) => println!("find_object unexpectedly OK len={}", o.data.len()),
                Err(e) => println!("find_object refused: {e}"),
            }
            println!("rss_kib after find_object={}", rss_kib());
        }
        _ => unreachable!(),
    }
}

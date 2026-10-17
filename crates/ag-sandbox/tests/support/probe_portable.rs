use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize| args.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    match args.get(1).map(String::as_str).unwrap_or("") {
        "ids" => println!(
            "uid={} euid={} gid={} egid={} pid={}",
            unsafe { libc::getuid() },
            unsafe { libc::geteuid() },
            unsafe { libc::getgid() },
            unsafe { libc::getegid() },
            std::process::id()
        ),
        "euid" => println!("{}", unsafe { libc::geteuid() }),
        "env" => {
            let mut v: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
            v.sort();
            for l in v {
                println!("{l}");
            }
        }
        "sleep" => std::thread::sleep(Duration::from_millis(num(2))),
        "exit" => std::process::exit(num(2) as i32),
        other => {
            println!("err ENOSYS");
            eprintln!("probe: '{other}' is not available on this platform");
            std::process::exit(2);
        }
    }
}

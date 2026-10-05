fn main() {
    embed_internet_defaults();
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to embed the Windows executable icon");
    }
}

fn embed_internet_defaults() {
    const PATH: &str = "internet-defaults.env";
    println!("cargo:rerun-if-changed={PATH}");
    if std::env::var_os("CARGO_FEATURE_RENDEZVOUS").is_none() {
        return;
    }
    let text = match std::fs::read_to_string(PATH) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("cannot read {PATH}: {error}"),
    };
    let mut seen = std::collections::BTreeSet::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .expect("expected KEY=value in internet-defaults.env");
        let key = key.trim();
        assert!(
            matches!(
                key,
                "JIGSALL_RENDEZVOUS_WSS_URL"
                    | "JIGSALL_ICE_STUN_SERVERS"
                    | "JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES"
            ),
            "unknown internet default: {key}"
        );
        assert!(seen.insert(key), "duplicate internet default: {key}");
        println!("cargo:rustc-env=BUILTIN_{key}={}", value.trim());
    }
}

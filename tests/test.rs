use assert_cmd::cargo::CommandCargoExt;
use core::time::Duration;
use std::{
    io::{self, Read},
    process::{Command, Stdio},
    thread::sleep,
};

#[test]
fn happy_3_peers() -> io::Result<()> {
    let mut children = [
        (8080, None, 5),
        (8081, Some(8080), 6),
        (8082, Some(8080), 7),
    ]
    .map(|(port, connect, period)| {
        let mut cmd = Command::cargo_bin("p2p-gossip").unwrap();
        cmd.args([
            "--skip-server-verification",
            &format!("--period={period}"),
            &format!("--port={port}"),
        ]);
        if let Some(connect_port) = connect {
            cmd.arg(format!("--connect=127.0.0.1:{connect_port}"));
        }
        let child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        sleep(Duration::from_millis(100));
        child
    });

    sleep(Duration::from_secs(16));

    for child in &mut children {
        let mut kill = Command::new("kill")
            .args(["-s", "SIGINT", &child.id().to_string()])
            .spawn()?;
        kill.wait()?;

        let mut err = String::new();
        child.stderr.take().unwrap().read_to_string(&mut err)?;
        assert_eq!(err, "");
    }

    let outs = children.map(|mut child| {
        let mut out = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        out
    });

    let mut lines = [outs[0].lines(), outs[1].lines(), outs[2].lines()];

    // launch

    for (i, port) in [8080, 8081, 8082].iter().enumerate() {
        let line = lines[i].next().expect("expected a line");
        assert_eq!(
            line,
            format!("00:00:00 - My address is \"127.0.0.1:{port}\"")
        );
    }

    // peers connecting

    let line = lines[0].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:00 - Accepted a connection from 127.0.0.1:808"),
        "bad line:\n{line}"
    );
    let first_was_8081 = line.ends_with("1");

    let line = lines[0].next().expect("expected a line");
    if first_was_8081 {
        assert_eq!(line, "00:00:00 - Accepted a connection from 127.0.0.1:8082");
    } else {
        assert_eq!(line, "00:00:00 - Accepted a connection from 127.0.0.1:8081");
    }

    let line = lines[1].next().expect("expected a line");
    assert_eq!(
        line,
        "00:00:00 - Connected to the peers at [\"127.0.0.1:8080\"]"
    );

    let line = lines[2].next().expect("expected a line");
    assert!(
        line == "00:00:00 - Connected to the peers at [\"127.0.0.1:8080\", \"127.0.0.1:8081\"]"
            || line
                == "00:00:00 - Connected to the peers at [\"127.0.0.1:8081\", \"127.0.0.1:8080\"]",
        "bad line:\n{line}"
    );

    let line = lines[1].next().expect("expected a line");
    assert_eq!(line, "00:00:00 - Accepted a connection from 127.0.0.1:8082");

    // 8080 sends a message

    let line = lines[0].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:05 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8081\", \"127.0.0.1:8082\"]")
                || line.ends_with("] to [\"127.0.0.1:8082\", \"127.0.0.1:8081\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [1, 2] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8080")),
            "bad line:\n{line}"
        );
    }

    // 8081 sends a message

    let line = lines[1].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:06 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8080\", \"127.0.0.1:8082\"]")
                || line.ends_with("] to [\"127.0.0.1:8082\", \"127.0.0.1:8080\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [0, 2] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8081")),
            "bad line:\n{line}"
        );
    }

    // 8082 sends a message

    let line = lines[2].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:07 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8080\", \"127.0.0.1:8081\"]")
                || line.ends_with("] to [\"127.0.0.1:8081\", \"127.0.0.1:8080\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [0, 1] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8082")),
            "bad line:\n{line}"
        );
    }

    // 8080 sends a message

    let line = lines[0].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:10 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8081\", \"127.0.0.1:8082\"]")
                || line.ends_with("] to [\"127.0.0.1:8082\", \"127.0.0.1:8081\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [1, 2] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8080")),
            "bad line:\n{line}"
        );
    }

    // 8081 sends a message

    let line = lines[1].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:12 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8080\", \"127.0.0.1:8082\"]")
                || line.ends_with("] to [\"127.0.0.1:8082\", \"127.0.0.1:8080\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [0, 2] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8081")),
            "bad line:\n{line}"
        );
    }

    // 8082 sends a message

    let line = lines[2].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:14 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8080\", \"127.0.0.1:8081\"]")
                || line.ends_with("] to [\"127.0.0.1:8081\", \"127.0.0.1:8080\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [0, 1] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8082")),
            "bad line:\n{line}"
        );
    }

    // 8080 sends a message

    let line = lines[0].next().expect("expected a line");
    assert!(
        line.starts_with("00:00:15 - Sending message [")
            && (line.ends_with("] to [\"127.0.0.1:8081\", \"127.0.0.1:8082\"]")
                || line.ends_with("] to [\"127.0.0.1:8082\", \"127.0.0.1:8081\"]")),
        "bad line:\n{line}"
    );
    let msg = extract_message(line);

    for l in [1, 2] {
        let line = lines[l].next().expect("expected a line");
        assert!(
            line.ends_with(&format!("Received message [{msg}] from 127.0.0.1:8080")),
            "bad line:\n{line}"
        );
    }

    // shutdown

    let line = lines[0].next().expect("expected a line");
    assert!(line.ends_with("Shutting down"), "bad line:\n{line}");
    let line = lines[0].next().expect("expected a line");
    let closed_8081 = line.ends_with("Closed connection to 127.0.0.1:8081, reason: closed");
    if !closed_8081 {
        assert!(
            line.ends_with("Closed connection to 127.0.0.1:8082, reason: closed"),
            "bad line:\n{line}"
        );
    }
    let line = lines[0].next().expect("expected a line");
    if closed_8081 {
        assert!(
            line.ends_with("Closed connection to 127.0.0.1:8082, reason: closed"),
            "bad line:\n{line}"
        );
    } else {
        assert!(
            line.ends_with("Closed connection to 127.0.0.1:8081, reason: closed"),
            "bad line:\n{line}"
        );
    }

    let line = lines[1].next().expect("expected a line");
    assert!(
        line.ends_with(
            "Closed connection to 127.0.0.1:8080, reason: closed by peer: shutdown (code 2)"
        ),
        "bad line:\n{line}"
    );
    let line = lines[1].next().expect("expected a line");
    assert!(line.ends_with("Shutting down"), "bad line:\n{line}");
    let line = lines[1].next().expect("expected a line");
    assert!(
        line.ends_with("Closed connection to 127.0.0.1:8082, reason: closed"),
        "bad line:\n{line}"
    );

    let line = lines[2].next().expect("expected a line");
    assert!(
        line.ends_with(
            "Closed connection to 127.0.0.1:8080, reason: closed by peer: shutdown (code 2)"
        ),
        "bad line:\n{line}"
    );
    let line = lines[2].next().expect("expected a line");
    assert!(
        line.ends_with(
            "Closed connection to 127.0.0.1:8081, reason: closed by peer: shutdown (code 2)"
        ),
        "bad line:\n{line}"
    );
    let line = lines[2].next().expect("expected a line");
    assert!(line.ends_with("Shutting down"), "bad line:\n{line}");

    for lines in &mut lines {
        assert!(lines.next().is_none());
    }

    Ok(())
}

fn extract_message(s: &str) -> &str {
    let start = s.bytes().position(|x| x == b'[').unwrap();
    let end = s.bytes().position(|x| x == b']').unwrap();
    &s[start + 1..end]
}

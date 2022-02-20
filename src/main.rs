use self::{
    event::EventLoop,
    models::{YabaiSpace, YabaiWindow},
};
use crate::constants::{QUERY_CURRENT_SPACE, QUERY_SPACE_WINDOWS};
use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use std::{
    env,
    fmt::Debug,
    io::{Read, Write},
    os::unix::net::UnixStream,
};
mod constants;
mod event;
mod models;

fn main() -> Result<()> {
    let mut args: Vec<String> = env::args().collect();
    args.remove(0); // Remove caller from argument.
    if args.len() < 2 {
        if args[0] == "watch" {
            EventLoop::start()?
        } else {
            bail!("yctrl: Not enough arguments provided.")
        }
    }

    // Get yabai scoket path
    let user = env::var("USER")?;
    let socket_path = format!("/tmp/yabai_{user}.socket");

    // Fix when the user provided id for sub command.
    let mut command_pos = 1;
    if args[1].as_str().parse::<u32>().is_ok() {
        command_pos = 2;
    }

    // Correct format: Note should maybe check if it's already correct
    let command = args.get_mut(command_pos).unwrap();
    let cmd = command.clone();
    *command = format!("--{command}");

    // Check if we should just redirect to yabai scoket.
    if should_just_redirect(&cmd, &args) {
        println!("redircting '{:?}' to yabai socket.", args);
        return execute(&socket_path, &args);
    }

    // Handle User request
    match args[0].as_str() {
        "window" => Window::handle(socket_path, args),
        "space" => Space::handle(socket_path, args),
        _ => execute(&socket_path, &args).map(|_| ()),
    }
}

fn should_just_redirect<A: AsRef<[u8]> + Debug>(cmd: &str, _args: &[A]) -> bool {
    cmd != "focus"
        && cmd != "swap"
        && cmd != "move"
        && cmd != "warp"
        && cmd != "space"
        && cmd != "inc"
        && cmd != "make"
}

struct Window();
impl Window {
    fn space(socket_path: String, args: Vec<String>) -> Result<()> {
        let select = args.last().unwrap();
        let command = args[1].clone();
        let space_args = vec!["space".to_string(), "--focus".to_string(), select.clone()];

        // Only further process next/prev, if not run the command as it.
        if select != "next" && select != "prev" && execute(&socket_path, &args).is_ok() {
            return Space::handle(socket_path, space_args);
        }

        // Try to execute as is
        if execute(&socket_path, &args).is_ok() {
            return Space::handle(socket_path, space_args);
        }

        // Try position rather than order
        let pos = if select == "next" { "first" } else { "last" };
        if execute(&socket_path, &["window", &command, pos]).is_ok() {
            return Space::handle(socket_path, space_args);
        }

        bail!("Fail handle space command!!! {:?}", args)
    }

    /// Toggle between largest and smallest window.
    /// TODO: Switch between left space and child windows
    fn master(socket_path: String) -> Result<()> {
        execute(&socket_path, &["window", "--warp", "first"])
            .or_else(|_| execute(&socket_path, &["window", "--warp", "last"]))
        // let windows: Vec<YabaiWindow> = query(&socket_path, QUERY_SPACE_WINDOWS)?;
        // let current = windows.iter().find(|w| w.has_focus).unwrap();
        // let largest = windows.iter().max_by_key(|&w| w.frame.sum()).unwrap();
        // eprintln!("largest = {:#?}", largest);
        // eprintln!("total = {:#?}", largest.frame.sum());
        // let mut partial_args = vec!["window".to_string(), "--warp".to_string()];
        // if largest.id == current.id {
        //     partial_args.push("next".to_string());
        //     Self::handle(socket_path, partial_args)
        // } else {
        //     partial_args.push(largest.id.to_string());
        //     Self::handle(socket_path, partial_args)
        // }
    }

    fn inc(socket_path: String, args: Vec<String>) -> Result<()> {
        let left = args.last().unwrap() == "left";
        let dir = if left { "-150:0" } else { "+150:0" };
        let args = &["window", "--resize", &format!("left:{dir}")];

        execute(&socket_path, args).or_else(|_| {
            let mut args = args.to_vec();
            let dir = format!("right:{dir}");
            args.insert(2, &dir);
            execute(&socket_path, &args)
        })
    }

    fn handle(socket_path: String, args: Vec<String>) -> Result<()> {
        // Handle special cases
        match (args[1].as_str(), args[2].as_str()) {
            ("--space", _) => return Self::space(socket_path, args),
            ("--inc", _) => return Self::inc(socket_path, args),
            ("--make", "master") => return Self::master(socket_path),
            _ => (),
        };

        let select = args.last().unwrap().as_str();
        let command = args[1].clone();

        // Only further process next/prev, if not run the command as it.
        if select != "next" && select != "prev" {
            println!("got {select} redirecting to yabai socket");
            return execute(&socket_path, &args);
        }

        // See if next/prev just works before doing anything else.
        if execute(&socket_path, &args).is_ok() {
            println!("successfully ran {select} through yabai socket");
            return Ok(());
        }

        // Get current space information.
        let space = query::<YabaiSpace, _>(&socket_path, QUERY_CURRENT_SPACE)?;

        // Should just change focus to next space window
        // TODO: support moving window to next/prev space and delete current space empty??
        if space.first_window == space.last_window && &command == "--focus" {
            let windows = query::<Vec<YabaiWindow>, _>(&socket_path, QUERY_SPACE_WINDOWS)?
                .into_iter()
                // not sure why Hammerspoon create these windows
                .filter(|w| w.subrole != "AXUnknown.Hammerspoon" && w.is_visible && !w.has_focus)
                .collect::<Vec<YabaiWindow>>();

            if windows.is_empty() {
                println!("No windows left in space, trying {select} space instead of window");
                let args = vec!["space".to_string(), command, select.to_string()];
                return Space::handle(socket_path, args);
            } else {
                let args = &["window", &command, &windows.first().unwrap().id.to_string()];
                return execute(&socket_path, args);
            }
        }

        // Get Id based on whether the select value.
        let id = if select == "next" {
            space.first_window.to_string()
        } else {
            space.last_window.to_string()
        };

        println!("{select} window isn't found, trying to foucs {id}");

        // Finally, Try to focus by id or else focus to first window
        execute(&socket_path, &["window", &command, &id])
            .or_else(|_| execute(&socket_path, &["window", &command, "first"]))
    }
}

struct Space();
impl Space {
    fn handle(socket_path: String, args: Vec<String>) -> Result<()> {
        let select = args.last().unwrap();

        // Only further process when select != next/prev and succeeded
        if select != "next" && select != "prev" && execute(&socket_path, &args).is_ok() {
            return Ok(());
        }

        // See if next/prev just works before doing anything else.
        execute(&socket_path, &args).or_else(|_| {
            let pos = if select == "next" { "first" } else { "last" };
            execute(&socket_path, &["space", &args[1], pos])
        })
    }
}

/// Send request to yabai socket and return string.
pub fn request<A>(socket_path: &str, args: &[A]) -> Result<String>
where
    A: AsRef<[u8]> + Debug,
{
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_nonblocking(false)?;

    for arg in args.iter().map(AsRef::as_ref) {
        if arg.contains(&b'\0') {
            bail!("Internal: Unexpected NUL byte in arg: {arg:?}");
        }
        stream.write_all(arg)?;
        stream.write_all(b"\0")?;
    }

    stream.write_all(b"\0")?;
    stream.flush()?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;

    if buf.get(0) == Some(&7) {
        anyhow::bail!(
            "Yabai: {} {:?}",
            String::from_utf8_lossy(&buf[1..]).trim(),
            args
        );
    }

    Ok(String::from_utf8(buf)?)
}

/// Send request to yabai socket.
pub fn execute<A>(socket_path: &str, args: &[A]) -> Result<()>
where
    A: AsRef<[u8]> + Debug,
{
    let mut buf = [0; 1];
    let mut stream = UnixStream::connect(socket_path)?;

    stream.set_nonblocking(false)?;

    for arg in args.iter().map(AsRef::as_ref) {
        if arg.contains(&b'\0') {
            bail!("Internal: Unexpected NUL byte in arg: {arg:?}");
        }
        stream.write_all(arg)?;
        stream.write_all(b"\0")?;
    }

    stream.write_all(b"\0")?;
    stream.flush()?;

    // Ignore if yabai return nothing.
    stream.read_exact(&mut buf).ok();

    if buf.get(0) == Some(&7) {
        bail!("Yabai: fail to execute {:?}", args)
    }

    Ok(())
}

pub fn query<T, A>(socket_path: &str, args: &[A]) -> Result<T>
where
    T: DeserializeOwned,
    A: AsRef<[u8]> + Debug,
{
    loop {
        // NOTE: According to @slam, sometime queries return empty string.
        let raw = request(socket_path, args)?;
        if raw.is_empty() {
            eprintln!("{:?} returned an empty string, retrying", args);
            continue;
        }
        return serde_json::from_str(&raw)
            .with_context(|| format!("Failed to desrialize JSON: {raw}"));
    }
}

/// macOS-specific Claude CLI spawning that disclaims TCC responsibility.
///
/// When our app spawns a child process normally, macOS makes us the "responsible
/// process" for TCC: any permission dialog the child triggers (Photos, Desktop,
/// Downloads …) is attributed to Live Meeting Helper, even though we don't need
/// those permissions ourselves.
///
/// `responsibility_spawnattrs_setdisclaim(attr, 1)` is the same private-but-stable
/// libSystem API that Terminal.app calls before it forks a shell. It tells the
/// kernel "I am not the responsible party for this child." macOS then uses the
/// child's own code-signing identity (com.anthropic.claude-code) for TCC, so any
/// permission prompts show "Claude Code" and are remembered per that bundle ID —
/// completely separate from Live Meeting Helper.
use std::ffi::CString;

// Private libSystem API — present on all macOS versions that support hardened
// runtime (10.14+). Documented in open-source XNU and used by Terminal / Xcode.
extern "C" {
    fn responsibility_spawnattrs_setdisclaim(
        attr: *mut libc::posix_spawnattr_t,
        disclaim: libc::c_int,
    ) -> libc::c_int;
}

/// Run `cli_path -p` with `prompt` on stdin, returning stdout as a String.
/// Blocks the calling thread; call from `tokio::task::spawn_blocking`.
pub fn run_claude_disclaimed(
    cli_path: &str,
    prompt: &str,
    expanded_path: &str,
) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::os::unix::io::FromRawFd;

    let path_c =
        CString::new(cli_path).map_err(|e| format!("invalid cli path: {e}"))?;
    let arg_p = CString::new("-p").unwrap();
    // posix_spawn does NOT do PATH lookup (unlike execvp). Use /usr/bin/env as
    // the executable — it always lives at that fixed path and performs lookup
    // using the PATH we inject into the child environment.
    let env_exec = CString::new("/usr/bin/env").unwrap();

    // Build environment: inherit everything, override PATH.
    let mut env_strings: Vec<CString> = std::env::vars()
        .filter(|(k, _)| k != "PATH")
        .map(|(k, v)| CString::new(format!("{k}={v}")).unwrap())
        .collect();
    env_strings.push(CString::new(format!("PATH={expanded_path}")).unwrap());

    // argv: ["/usr/bin/env", "claude", "-p", NULL]
    let argv_ptrs: Vec<*const libc::c_char> = vec![
        env_exec.as_ptr(),
        path_c.as_ptr(),
        arg_p.as_ptr(),
        std::ptr::null(),
    ];
    let envp_ptrs: Vec<*const libc::c_char> = env_strings
        .iter()
        .map(|s| s.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    unsafe {
        // ── stdin pipe (parent writes prompt → child reads) ───────────────
        let mut stdin_fds = [0i32; 2];
        if libc::pipe(stdin_fds.as_mut_ptr()) != 0 {
            return Err("pipe(stdin) failed".into());
        }
        let (stdin_r, stdin_w) = (stdin_fds[0], stdin_fds[1]);

        // ── stdout pipe (child writes → parent reads) ─────────────────────
        let mut stdout_fds = [0i32; 2];
        if libc::pipe(stdout_fds.as_mut_ptr()) != 0 {
            libc::close(stdin_r);
            libc::close(stdin_w);
            return Err("pipe(stdout) failed".into());
        }
        let (stdout_r, stdout_w) = (stdout_fds[0], stdout_fds[1]);

        // ── stderr pipe ───────────────────────────────────────────────────
        let mut stderr_fds = [0i32; 2];
        if libc::pipe(stderr_fds.as_mut_ptr()) != 0 {
            libc::close(stdin_r);
            libc::close(stdin_w);
            libc::close(stdout_r);
            libc::close(stdout_w);
            return Err("pipe(stderr) failed".into());
        }
        let (stderr_r, stderr_w) = (stderr_fds[0], stderr_fds[1]);

        // ── file actions: wire child's 0/1/2 to the pipes ─────────────────
        let mut fa: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        libc::posix_spawn_file_actions_init(&mut fa);
        libc::posix_spawn_file_actions_adddup2(&mut fa, stdin_r, 0);
        libc::posix_spawn_file_actions_adddup2(&mut fa, stdout_w, 1);
        libc::posix_spawn_file_actions_adddup2(&mut fa, stderr_w, 2);
        // Close the parent-side ends inside the child.
        libc::posix_spawn_file_actions_addclose(&mut fa, stdin_w);
        libc::posix_spawn_file_actions_addclose(&mut fa, stdout_r);
        libc::posix_spawn_file_actions_addclose(&mut fa, stderr_r);

        // ── spawn attributes: disclaim TCC responsibility ─────────────────
        let mut sa: libc::posix_spawnattr_t = std::mem::zeroed();
        libc::posix_spawnattr_init(&mut sa);
        // Disclaim: child's own bundle ID (com.anthropic.claude-code) becomes
        // the responsible process for TCC instead of Live Meeting Helper.
        responsibility_spawnattrs_setdisclaim(&mut sa, 1);

        // ── spawn ─────────────────────────────────────────────────────────
        let mut pid: libc::pid_t = 0;
        let rc = libc::posix_spawn(
            &mut pid,
            env_exec.as_ptr(),
            &fa,
            &sa,
            argv_ptrs.as_ptr() as *mut *mut libc::c_char,
            envp_ptrs.as_ptr() as *mut *mut libc::c_char,
        );

        libc::posix_spawnattr_destroy(&mut sa);
        libc::posix_spawn_file_actions_destroy(&mut fa);

        if rc != 0 {
            libc::close(stdin_r);
            libc::close(stdin_w);
            libc::close(stdout_r);
            libc::close(stdout_w);
            libc::close(stderr_r);
            libc::close(stderr_w);
            return Err(format!(
                "posix_spawn failed (err={rc}): {}",
                std::io::Error::from_raw_os_error(rc)
            ));
        }

        // Close child-side ends in parent.
        libc::close(stdin_r);
        libc::close(stdout_w);
        libc::close(stderr_w);

        // ── write prompt → stdin, then close so child sees EOF ────────────
        {
            let mut f = std::fs::File::from_raw_fd(stdin_w);
            f.write_all(prompt.as_bytes())
                .map_err(|e| format!("write to claude stdin: {e}"))?;
            // drop closes stdin_w
        }

        // ── read stdout ───────────────────────────────────────────────────
        let stdout_str = {
            let mut f = std::fs::File::from_raw_fd(stdout_r);
            let mut buf = String::new();
            f.read_to_string(&mut buf)
                .map_err(|e| format!("read claude stdout: {e}"))?;
            buf
        };

        // ── read stderr (for error reporting) ────────────────────────────
        let stderr_str = {
            let mut f = std::fs::File::from_raw_fd(stderr_r);
            let mut buf = String::new();
            let _ = f.read_to_string(&mut buf);
            buf
        };

        // ── wait for child ────────────────────────────────────────────────
        let mut status: libc::c_int = 0;
        libc::waitpid(pid, &mut status, 0);

        let exit_ok = libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0;
        if !exit_ok {
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                -1
            };
            return Err(format!(
                "claude CLI exited with code {code}: {stderr_str}"
            ));
        }

        Ok(stdout_str.trim().to_string())
    }
}

mod cli;
#[cfg(feature = "gui")]
mod gui;
mod session;
#[cfg(test)]
mod tests;

pub use cli::{
    CliInputEvent, execute_command, parse_account_id_arg, render_dashboard_string, run_cli_loop,
    spawn_input_reader,
};
pub use session::AppSession;

use std::io::{self, Write};
use truesight_adapters::gsi::BoundedGsiListener;
use truesight_adapters::sync::LocalCacheManager;
use truesight_adapters::vision::BoundedVisionWorker;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut session = match AppSession::new() {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(io::stderr(), "Failed to initialize truesight-dota: {e}");
            std::process::exit(1);
        }
    };
    match parse_account_id_arg(&args) {
        Ok(Some(opt_id)) => session.set_account_id(opt_id, true),
        Ok(None) => {}
        Err(e) => {
            let _ = writeln!(io::stderr(), "{e}");
            std::process::exit(1);
        }
    }

    if args.iter().any(|a| a == "--sync") {
        match session.sync_db(true) {
            Ok(msg) => {
                let _ = writeln!(io::stdout(), "{msg} ({})", session.db_status);
                return;
            }
            Err(e) => {
                let _ = writeln!(io::stderr(), "Sync failed: {e}");
                std::process::exit(1);
            }
        }
    }

    if args.iter().any(|a| a == "--demo") {
        let _ = execute_command(&mut session, "ally 3 magnus");
        let _ = execute_command(&mut session, "ally 5 crystal maiden");
        let _ = execute_command(&mut session, "enemy 1 medusa");
        let _ = execute_command(&mut session, "enemy 2 storm spirit");
        let _ = execute_command(&mut session, "enemy 3 axe");
        let _ = execute_command(&mut session, "role 1");
        let no_color = args.iter().any(|a| a == "--no-color");
        if writeln!(io::stdout(), "{}", render_dashboard_string(&session, !no_color)).is_err() {
            std::process::exit(0);
        }
        return;
    }

    // Background non-blocking auto-sync when cache TTL (24h meta / 1h player) has expired
    {
        let now = LocalCacheManager::now_unix();
        if session.cache_mgr.is_meta_stale(now) || session.cache_mgr.is_player_stale(now) {
            let _ = session.start_background_sync(false);
        }
    }

    let gsi_addr = std::env::var("TRUESIGHT_GSI_ADDR").unwrap_or_else(|_| "127.0.0.1:4001".into());
    let mut gsi_listener = match BoundedGsiListener::bind(&gsi_addr) {
        Ok(l) => {
            session.last_gsi_state = format!("LISTENING on http://{}", l.local_addr());
            Some(l)
        }
        Err(e) => {
            session.last_gsi_state = format!("GSI DISABLED ({e})");
            let _ = writeln!(io::stderr(), "Warning: GSI listener disabled: {e}");
            None
        }
    };
    let mut vision_worker = BoundedVisionWorker::spawn();

    #[cfg(feature = "gui")]
    {
        let force_cli = args.iter().any(|a| a == "--cli" || a == "--tui");
        if !force_cli {
            if let Err(e) = gui::run_gui(session, gsi_listener, vision_worker) {
                let _ = writeln!(io::stderr(), "GUI error: {e}");
                std::process::exit(1);
            }
            return;
        }
    }

    let input_rx = spawn_input_reader(io::stdin());
    let mut stdout = io::stdout();
    let _ = run_cli_loop(
        &mut session,
        gsi_listener.as_mut(),
        vision_worker.as_mut(),
        &input_rx,
        &mut stdout,
        true,
        std::time::Duration::from_millis(100),
    );
}

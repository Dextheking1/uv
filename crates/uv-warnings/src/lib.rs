use std::error::Error;
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{LazyLock, Mutex};

// macro hygiene: The user might not have direct dependencies on those crates
#[doc(hidden)]
pub use anstream;
#[doc(hidden)]
pub use owo_colors;
use rustc_hash::FxHashSet;
#[doc(hidden)]
pub use uv_errors::Hints;
use uv_errors::{ErrorOptions, write_error_chain_with_options};

/// Whether user-facing warnings are enabled.
pub static ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable user-facing warnings.
pub fn enable() {
    ENABLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Disable user-facing warnings.
pub fn disable() {
    ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// A callback function for printing warnings.
type PrinterCallback = Box<dyn Fn(&str) + Send + Sync>;

/// A global printer callback that, when set, is used to print warnings instead of writing
/// directly to stderr. This allows coordinating warning output with indicatif's progress bar
/// system via `MultiProgress::suspend()`, so active progress bars can't truncate warnings.
/// See: <https://github.com/astral-sh/uv/issues/18626>.
static PRINTER: Mutex<Option<PrinterCallback>> = Mutex::new(None);

/// Set a global printer callback for warning output.
///
/// When set, all warnings are routed through this callback instead of writing directly
/// to stderr. This is used to coordinate with indicatif progress bars.
///
/// Note: only one printer callback can be active at a time. If multiple reporters call
/// `set_printer` concurrently, the last one wins and `clear_printer` from an earlier
/// reporter will remove the later reporter's callback. Callers should ensure only one
/// reporter is active at a time.
pub fn set_printer(callback: PrinterCallback) {
    if let Ok(mut printer) = PRINTER.lock() {
        *printer = Some(callback);
    }
}

/// Clear the global printer callback, restoring direct stderr output for warnings.
pub fn clear_printer() {
    if let Ok(mut printer) = PRINTER.lock() {
        *printer = None;
    }
}

/// Print a warning message, routing through the global printer callback if one is set,
/// or falling back to writing directly to stderr.
///
/// The message is written as-is, and is expected to end with a newline.
///
/// Uses `try_lock()` instead of `lock()` to avoid deadlocking if the callback
/// (or anything it transitively calls) triggers another warning on the same thread.
#[doc(hidden)]
pub fn print_warning(line: &str) {
    if let Ok(printer) = PRINTER.try_lock() {
        if let Some(callback) = printer.as_ref() {
            callback(line);
            return;
        }
    }
    anstream::eprint!("{line}");
}

/// Format a warning chain to standard error.
pub fn write_warning_chain(err: &dyn Error, hints: &Hints<'_>) -> fmt::Result {
    let mut message = String::new();
    write_warning_chain_with_options(
        err,
        hints,
        ErrorOptions::default().with_stream(&mut message),
    )?;
    print_warning(&message);
    Ok(())
}

/// Format a warning chain to standard error once, deduplicating the complete rendered chain and hints.
pub fn write_warning_chain_once(err: &dyn Error, hints: &Hints<'_>) -> fmt::Result {
    let mut message = String::new();
    write_warning_chain_with_options(
        err,
        hints,
        ErrorOptions::default().with_stream(&mut message),
    )?;
    if let Ok(mut warnings) = WARNINGS.lock()
        && warnings.insert(message.clone())
    {
        print_warning(&message);
    }
    Ok(())
}

fn write_warning_chain_with_options<C, W: fmt::Write>(
    err: &dyn Error,
    hints: &Hints<'_>,
    options: ErrorOptions<'_, C, W>,
) -> fmt::Result {
    write_error_chain_with_options(
        err,
        hints,
        options
            .with_level("warning")
            .with_color(owo_colors::AnsiColors::Yellow),
    )
}

/// Warn a user, if warnings are enabled.
#[macro_export]
macro_rules! warn_user {
    ($($arg:tt)*) => {{
        use $crate::owo_colors::OwoColorize;

        if $crate::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            let message = format!("{}", format_args!($($arg)*));
            let formatted = message.bold();
            let line = format!("{}{} {formatted}\n", "warning".yellow().bold(), ":".bold());
            $crate::print_warning(&line);
        }
    }};
}

/// Warn a user with an error and its cause chain, if warnings are enabled.
///
/// The error must be passed as a reference to a type implementing [`Error`], or as a
/// `&dyn Error`. Optional [`Hints`] are rendered after the cause chain. Arguments are
/// only evaluated when warnings are enabled.
///
/// Attach context to the error to include a warning-specific message without losing its causes:
///
/// ```
/// # let source = std::io::Error::other("invalid script metadata");
/// uv_warnings::warn_user_with_chain!(
///     anyhow::Error::from(source)
///         .context("Skipping invalid PEP 723 script `script.py`")
///         .as_ref()
/// );
/// ```
#[macro_export]
macro_rules! warn_user_with_chain {
    ($err:expr $(,)?) => {
        $crate::warn_user_with_chain!($err, $crate::Hints::none())
    };
    ($err:expr, $hints:expr $(,)?) => {{
        if $crate::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            $crate::write_warning_chain($err, &$hints).expect("writing to stderr should not fail");
        }
    }};
}

pub static WARNINGS: LazyLock<Mutex<FxHashSet<String>>> = LazyLock::new(Mutex::default);

/// Warn a user once, if warnings are enabled, with uniqueness determined by the content of the
/// message.
#[macro_export]
macro_rules! warn_user_once {
    ($($arg:tt)*) => {{
        use $crate::owo_colors::OwoColorize;

        if $crate::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(mut states) = $crate::WARNINGS.lock() {
                let message = format!("{}", format_args!($($arg)*));
                if states.insert(message.clone()) {
                    let line = format!("{}{} {}\n", "warning".yellow().bold(), ":".bold(), message.bold());
                    $crate::print_warning(&line);
                }
            }
        }
    }};
}

/// Warn a user once with an error and its cause chain, if warnings are enabled.
///
/// Accepts the same arguments as [`warn_user_with_chain!`]. Uniqueness is determined
/// by the complete rendered chain and hints, so distinct causes are not suppressed.
#[macro_export]
macro_rules! warn_user_once_with_chain {
    ($err:expr $(,)?) => {
        $crate::warn_user_once_with_chain!($err, $crate::Hints::none())
    };
    ($err:expr, $hints:expr $(,)?) => {{
        if $crate::ENABLED.load(std::sync::atomic::Ordering::Relaxed) {
            $crate::write_warning_chain_once($err, &$hints)
                .expect("writing to stderr should not fail");
        }
    }};
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;
    use std::sync::{Arc, Mutex};

    use anyhow::anyhow;
    use insta::assert_snapshot;
    use rustc_hash::FxHashSet;
    use uv_errors::{ErrorOptions, Hints};

    use super::{
        clear_printer, disable, print_warning, set_printer, write_warning_chain_with_options,
    };

    /// Test-only variant of [`super::write_warning_chain_once`] that writes to an
    /// in-memory writer instead of routing through the global printer.
    fn write_warning_chain_once_with_writer(
        err: &dyn Error,
        hints: &Hints<'_>,
        warnings: &Mutex<FxHashSet<String>>,
        mut writer: impl fmt::Write,
    ) -> fmt::Result {
        let mut message = String::new();
        write_warning_chain_with_options(
            err,
            hints,
            ErrorOptions::default().with_stream(&mut message),
        )?;
        if let Ok(mut warnings) = warnings.lock()
            && warnings.insert(message.clone())
        {
            writer.write_str(&message)?;
        }
        Ok(())
    }

    #[test]
    fn format_warning_chain() {
        let error = anyhow!("Failed to create registry entry");
        let mut output = String::new();
        write_warning_chain_with_options(
            error.as_ref(),
            &Hints::none(),
            ErrorOptions::default().with_stream(&mut output),
        )
        .unwrap();
        assert_snapshot!(format!("{output:?}"), @r#""\u{1b}[1m\u{1b}[33mwarning\u{1b}[39m\u{1b}[0m\u{1b}[1m:\u{1b}[0m Failed to create registry entry\n""#);
        let output = anstream::adapter::strip_str(&output);

        assert_snapshot!(output, @"warning: Failed to create registry entry
");
    }

    #[test]
    fn format_warning_with_causes_and_hints() {
        let error = anyhow!("Permission denied")
            .context("Failed to write registry entry")
            .context("Failed to install Python");
        let mut output = String::new();
        write_warning_chain_with_options(
            error.as_ref(),
            &Hints::from("Check the registry permissions."),
            ErrorOptions::default().with_stream(&mut output),
        )
        .unwrap();
        assert_snapshot!(format!("{output:?}"), @r#""\u{1b}[1m\u{1b}[33mwarning\u{1b}[39m\u{1b}[0m\u{1b}[1m:\u{1b}[0m Failed to install Python\n  \u{1b}[1m\u{1b}[33mcause\u{1b}[39m\u{1b}[0m\u{1b}[1m:\u{1b}[0m Failed to write registry entry\n  \u{1b}[1m\u{1b}[33mcause\u{1b}[39m\u{1b}[0m\u{1b}[1m:\u{1b}[0m Permission denied\n\n\u{1b}[36m\u{1b}[1mhint\u{1b}[0m\u{1b}[39m\u{1b}[1m:\u{1b}[0m Check the registry permissions.\n""#);
        let output = anstream::adapter::strip_str(&output);

        assert_snapshot!(output, @"
        warning: Failed to install Python
          cause: Failed to write registry entry
          cause: Permission denied

        hint: Check the registry permissions.
        ");
    }

    #[test]
    fn format_warning_chain_once_preserves_distinct_causes_and_hints() -> fmt::Result {
        let warnings = Mutex::default();
        let mut output = String::new();
        for (cause, hint) in [
            ("Permission denied", "Unlock the keyring."),
            ("Permission denied", "Unlock the keyring."),
            ("Storage unavailable", "Unlock the keyring."),
            ("Permission denied", "Try another backend."),
        ] {
            let error = anyhow!(cause).context("Failed to read credentials");
            write_warning_chain_once_with_writer(
                error.as_ref(),
                &Hints::from(hint),
                &warnings,
                &mut output,
            )?;
        }
        let output = anstream::adapter::strip_str(&output);
        assert_snapshot!(output, @r"
        warning: Failed to read credentials
          cause: Permission denied

        hint: Unlock the keyring.
        warning: Failed to read credentials
          cause: Storage unavailable

        hint: Unlock the keyring.
        warning: Failed to read credentials
          cause: Permission denied

        hint: Try another backend.
        ");
        Ok(())
    }

    #[test]
    fn warn_user_with_chain_skips_disabled_arguments() {
        disable();
        let error = anyhow!("should not be displayed");
        let mut evaluations = 0;

        warn_user_with_chain!({
            evaluations += 1;
            error.as_ref()
        });
        warn_user_with_chain!(
            {
                evaluations += 1;
                error.as_ref()
            },
            {
                evaluations += 1;
                Hints::none()
            },
        );
        warn_user_once_with_chain!({
            evaluations += 1;
            error.as_ref()
        });
        warn_user_once_with_chain!(
            {
                evaluations += 1;
                error.as_ref()
            },
            {
                evaluations += 1;
                Hints::none()
            },
        );

        assert_eq!(evaluations, 0);
    }

    /// Serializes tests that mutate the global printer callback, since tests
    /// in the same binary run in parallel.
    static PRINTER_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Collect lines passed through the global printer callback.
    fn capture_printer() -> (Arc<Mutex<Vec<String>>>, impl FnOnce()) {
        let captured: Arc<Mutex<Vec<String>>> = Arc::default();
        let captured_clone = Arc::clone(&captured);
        set_printer(Box::new(move |line: &str| {
            captured_clone.lock().unwrap().push(line.to_string());
        }));
        let guard = || clear_printer();
        (captured, guard)
    }

    #[test]
    fn set_printer_routes_print_warning_through_callback() {
        let _lock = PRINTER_TEST_LOCK.lock().unwrap();
        let (captured, unguard) = capture_printer();
        print_warning("warning: something happened\n");
        unguard();

        let captured = captured.lock().unwrap();
        assert_eq!(*captured, ["warning: something happened\n"]);
    }

    #[test]
    fn clear_printer_stops_routing_through_callback() {
        let _lock = PRINTER_TEST_LOCK.lock().unwrap();
        let (captured, unguard) = capture_printer();
        unguard();
        // With no printer set, `print_warning` falls back to writing directly
        // to stderr, so the callback must not observe the message.
        print_warning("warning: not captured\n");

        let captured = captured.lock().unwrap();
        assert!(captured.is_empty());
    }

    #[test]
    fn set_printer_replaces_previous_callback() {
        let _lock = PRINTER_TEST_LOCK.lock().unwrap();
        let (first, unguard_first) = capture_printer();
        let (second, unguard_second) = capture_printer();
        print_warning("warning: latest wins\n");
        unguard_first();
        unguard_second();

        assert!(first.lock().unwrap().is_empty());
        assert_eq!(*second.lock().unwrap(), ["warning: latest wins\n"]);
    }

    #[test]
    fn reentrant_print_warning_does_not_deadlock() {
        let _lock = PRINTER_TEST_LOCK.lock().unwrap();
        // If the callback transitively triggers another warning, `print_warning`
        // must not deadlock on the printer lock; it falls back to stderr instead.
        set_printer(Box::new(|line: &str| {
            // The inner call cannot acquire the printer lock, so it writes
            // directly to stderr rather than recursing.
            if !line.contains("inner") {
                print_warning("warning: inner\n");
            }
        }));
        print_warning("warning: outer\n");
        clear_printer();
    }
}

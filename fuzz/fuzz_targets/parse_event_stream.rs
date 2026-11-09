#![no_main]

use libfuzzer_sys::fuzz_target;

// The NDJSON journal parser is pure (bytes in, records out, no I/O),
// so it is the fuzz surface for the stream-json format: any input
// must parse or be skipped, never panic.
fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = zen_cli::eventlog::parse_event_stream(text);
    }
});

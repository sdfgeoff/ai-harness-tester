use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
    sync::{Arc, Mutex},
    thread,
};

pub fn copy_output<R>(
    reader: R,
    log: Arc<Mutex<File>>,
    run_id: String,
) -> thread::JoinHandle<Result<(), String>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            let bytes_read = reader
                .read_until(b'\n', &mut buffer)
                .map_err(|error| format!("failed to read docker output: {error}"))?;
            if bytes_read == 0 {
                return Ok(());
            }

            {
                let mut log = log
                    .lock()
                    .map_err(|_| "harness log lock poisoned".to_owned())?;
                log.write_all(&buffer)
                    .map_err(|error| format!("failed to write harness log: {error}"))?;
            }

            write_prefixed_console_line(&run_id, &buffer)?;
        }
    })
}

fn write_prefixed_console_line(run_id: &str, line: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(format!("[{run_id}] ").as_bytes())
        .and_then(|_| stdout.write_all(line))
        .map_err(|error| format!("failed to write harness console output: {error}"))?;

    if !line.ends_with(b"\n") {
        stdout
            .write_all(b"\n")
            .map_err(|error| format!("failed to write harness console newline: {error}"))?;
    }

    stdout
        .flush()
        .map_err(|error| format!("failed to flush harness console output: {error}"))
}

pub fn join_log_thread(handle: thread::JoinHandle<Result<(), String>>) -> Result<(), String> {
    handle
        .join()
        .map_err(|_| "harness log thread panicked".to_owned())?
}

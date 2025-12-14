//! Lightweight Rust client for controlling a JSBSim instance over TCP.
//!
//! This module provides a small, focused API to spawn or connect to a
//! JSBSim process and interact with its console via the TCP interface.
//! It is intended for integration tests and tooling that need programmatic
//! control of JSBSim (get/set properties, step the simulation, hold/resume,
//! etc.). The implementation is intentionally minimal and synchronous.
//!
//! # Examples
//!
//! Spawn JSBSim and set a property:
//!
//! ```
//! use jsbsimrs::JSBSimProcessProperties;
//! use jsbsimrs::JSBSim;
//! let props = JSBSimProcessProperties::default();
//! let mut sim = JSBSim::new_with_process(props).expect("start jsbsim");
//! sim.set("fcs/throttle-cmd-norm", 1.0).unwrap();
//! ```
//!
//! Connect to an already-running JSBSim server:
//!
//! ```no_run
//! use jsbsimrs::JSBSim;
//! let mut sim = JSBSim::new("127.0.0.1:5556").expect("connect");
//! let t: f64 = sim.get("simulation/sim-time-sec").unwrap();
//! ```

use std::io::BufRead;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Stdio;

/// Configuration used when spawning a JSBSim process via
/// `JSBSim::new_with_process`.
///
/// Construct with `Default::default()` for a sensible local testing
/// configuration and override fields as necessary.
pub struct JSBSimProcessProperties {
    /// The name of the JSBSim executable
    executable_name: String,
    /// The JSBSim root directory
    root: PathBuf,
    /// The aircraft to load on start
    aircraft: Option<String>,
    /// The initialization script for the aircraft to run on start
    init_script: Option<String>,
    /// The script to run on start
    script: Option<String>,
    /// Low simulation rates can lead to unstable behavior or FP exceptions in JSBSim
    simulation_hz: i32,
    /// Run the simulation in a suspended state on start
    suspend_on_start: bool,
    /// Run the simulation in real time mode
    realtime: bool,
    /// The port to connect to JSBSim on
    port: u16,
}

impl Default for JSBSimProcessProperties {
    fn default() -> Self {
        JSBSimProcessProperties {
            executable_name: "JSBSim".to_string(),
            root: PathBuf::from("./jsbsim_root"),
            aircraft: Some("Concorde".to_string()),
            init_script: Some("reset00".to_string()),
            script: None,
            simulation_hz: 400,
            suspend_on_start: true,
            realtime: false,
            port: 5556,
        }
    }
}

/// A connected JSBSim client.
///
/// Holds the active TCP `connection` to the simulator console and an
/// optional owned `process` when the client started JSBSim itself. The
/// client exposes convenience methods to send console commands and parse
/// common responses.
pub struct JSBSim {
    connection: TcpStream,
    process: Option<std::process::Child>,
}

/// Error returned by `JSBSim::get` when retrieving a property value.
///
/// Wraps either an IO error or a parsing error while converting the
/// textual simulator response into the requested type `T`.
#[derive(Debug)]
pub enum GetError<T: std::str::FromStr + std::fmt::Debug>
where
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    /// Underlying IO error while communicating with JSBSim
    IoError(std::io::Error),
    /// Failed to parse the value returned by JSBSim into `T`
    ParseError(<T as std::str::FromStr>::Err),
}

impl<T: std::str::FromStr + std::fmt::Debug> From<std::io::Error> for GetError<T>
where
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    fn from(error: std::io::Error) -> Self {
        GetError::IoError(error)
    }
}

impl JSBSim {
    /// Connect to an already-running JSBSim TCP server at `address`.
    ///
    /// This returns a `JSBSim` instance which is ready to accept console
    /// commands. The function waits for the simulator prompt before
    /// returning.
    pub fn new(address: &str) -> std::io::Result<Self> {
        let stream = TcpStream::connect(address)?;
        let mut jsbsim = JSBSim {
            connection: stream,
            process: None,
        };
        jsbsim.read_line()?;
        Ok(jsbsim)
    }

    /// Spawn a new JSBSim process with the given `properties` and connect to it.
    ///
    /// This function spawns a new JSBSim process with the specified properties,
    /// waits for it to be ready, and then connects to its TCP console interface.
    /// It returns a `JSBSim` instance that can be used to interact with the
    /// simulator.
    pub fn new_with_process(properties: JSBSimProcessProperties) -> Result<Self, std::io::Error> {
        let mut command = std::process::Command::new(properties.executable_name.as_str());
        command
            .stdout(Stdio::piped())
            .arg(format!(
                "--simulation-rate={rate}",
                rate = properties.simulation_hz
            ))
            .arg(format!("--root={root}", root = properties.root.display()));

        if let Some(aircraft) = properties.aircraft {
            command.arg(format!("--aircraft={aircraft}", aircraft = aircraft));
        }

        if let Some(init_script) = properties.init_script {
            command.arg(format!("--initfile={script}", script = init_script));
        }

        if let Some(script) = properties.script {
            command.arg(format!("--script={script}", script = script));
        }

        if properties.suspend_on_start {
            command.arg("--suspend");
        }

        if properties.realtime {
            command.arg("--realtime");
        }

        let mut process = command.spawn()?;

        // Wait until JSBSim reports that it is ready to accept connections
        let stdout = process.stdout.as_mut().unwrap();
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break; // EOF
            }
            if line.contains("JSBSim Execution beginning") {
                break;
            }
        }

        let address = format!("localhost:{port}", port = properties.port);
        match TcpStream::connect(address) {
            Ok(stream) => {
                let mut jsbsim = JSBSim {
                    connection: stream,
                    process: Some(process),
                };
                jsbsim.read_line()?;
                return Ok(jsbsim);
            }
            Err(e) => {
                let _ = process.kill();
                let _ = process.wait();
                return Err(e);
            }
        }
    }

    /// Read one logical response line from the JSBSim console.
    fn read_line(&mut self) -> std::io::Result<String> {
        let mut reader = std::io::BufReader::new(&self.connection);
        let mut response = String::new();
        reader.read_line(&mut response)?;

        while response.trim().is_empty() || response.trim() == "JSBSim>" {
            response.clear();
            reader.read_line(&mut response)?;
        }
        Ok(response)
    }

    /// Ask JSBSim to enter the suspended "hold" state.
    pub fn hold(&mut self) -> std::io::Result<()> {
        self.send_command("hold\n")?;
        self.read_line().map(|_| ())
    }

    /// Resume simulation execution after a hold.
    pub fn resume(&mut self) -> std::io::Result<()> {
        self.send_command("resume\n")?;
        let line = self.read_line()?;
        if !line.trim().ends_with("Resuming") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to resume: {}", line.trim()),
            ));
        }
        Ok(())
    }

    /// Advance the simulation by `steps` iterations and verify success.
    pub fn iterate(&mut self, steps: i32) -> std::io::Result<()> {
        use std::io::Write;
        self.connection
            .write_all(format!("iterate {steps}\n", steps = steps).as_bytes())?;
        let line = self.read_line()?;
        if !line.trim().ends_with("Iterations performed") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to iterate: {}", line.trim()),
            ));
        }
        Ok(())
    }

    /// Set a simulator property `key` to `value`.
    ///
    /// The function asserts that JSBSim acknowledged the change with
    /// `set successful`.
    pub fn set(&mut self, key: &str, value: impl std::fmt::Display) -> std::io::Result<()> {
        use std::io::Write;
        self.connection
            .write_all(format!("set {key} {value}\n").as_bytes())?;
        let line = self.read_line()?;
        if !line.trim().ends_with("set successful") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to set property: {}", line.trim()),
            ));
        }
        Ok(())
    }

    /// Get the value of `key` from JSBSim and parse it into `T`.
    ///
    /// JSBSim replies with `key = value`; the RHS is parsed and returned or
    /// an error is produced if parsing fails.
    pub fn get<T: std::str::FromStr + std::fmt::Debug>(
        &mut self,
        key: &str,
    ) -> Result<T, GetError<T>>
    where
        <T as std::str::FromStr>::Err: std::fmt::Debug,
    {
        use std::io::Write;
        self.connection
            .write_all(format!("get {key}\n").as_bytes())?;
        let response = self.read_line()?;
        let parts = response.trim().split("=");
        let collection = parts.collect::<Vec<&str>>();
        debug_assert!(
            collection.len() == 2,
            "Response from JSBSim not in expected format '{}' '{}'",
            collection.len(),
            response.trim()
        );
        collection
            .get(1)
            .ok_or_else(|| {
                GetError::IoError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "No value returned",
                ))
            })?
            .trim()
            .parse::<T>()
            .map_err(GetError::ParseError)
    }

    /// Send a raw command string to JSBSim.
    fn send_command(&mut self, command: &str) -> std::io::Result<()> {
        use std::io::Write;
        self.connection.write_all(command.as_bytes())?;
        Ok(())
    }
}

impl Drop for JSBSim {
    /// Ensure the simulator is asked to quit and any spawned process is
    /// terminated and waited on. Errors are not propagated from `drop` but a
    /// warning is printed if the child did not exit cleanly.
    fn drop(&mut self) {
        let _ = self.send_command("quit\n");
        if let Some(mut process) = self.process.take() {
            process.kill().ok();
            let exit_code = process.wait();
            if !exit_code.map(|code| code.success()).unwrap_or(false) {
                eprintln!("Warning: JSBSim process did not exit cleanly");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn jsbsim_connection_performs_as_expected() {
        let properties = JSBSimProcessProperties {
            simulation_hz: 400,
            ..Default::default()
        };

        let mut jsbsim =
            JSBSim::new_with_process(properties).expect("Failed to start JSBSim process");
        /*
        let mut jsbsim = JSBSim::new("127.0.0.1:5556").expect("Failed to start JSBSim process");
        */

        // Able to get and set properties
        let time: i32 = jsbsim
            .get("simulation/cycle_duration")
            .expect("Failed to get time");
        assert_eq!(time, 0);
        let running_engine: i32 = jsbsim
            .get("propulsion/engine/set-running")
            .expect("Failed to get engine running state");
        assert_eq!(running_engine, 0);
        assert_eq!(
            jsbsim
                .get::<f64>("fcs/throttle-cmd-norm")
                .expect("Failed to get throttle"),
            0.0
        );
        jsbsim
            .set("fcs/throttle-cmd-norm", 1.0)
            .expect("Failed to set throttle");
        let throttle: f64 = jsbsim
            .get("fcs/throttle-cmd-norm")
            .expect("Failed to get throttle");
        assert_eq!(throttle, 1.0);

        // Time behaves as expected
        assert_eq!(
            jsbsim
                .get::<f64>("simulation/sim-time-sec")
                .expect("Failed to get time"),
            0.0025
        );
        jsbsim.iterate(120).expect("Failed to iterate");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(
            jsbsim
                .get::<f64>("simulation/sim-time-sec")
                .expect("Failed to get time"),
            0.3025
        );
        jsbsim.resume().expect("Failed to resume");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_ne!(
            jsbsim
                .get::<f64>("simulation/sim-time-sec")
                .expect("Failed to get time"),
            0.3025
        );
    }
}

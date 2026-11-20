//! The serial port, and the only place in the crate that talks to an OS device.
//!
//! Everything above this module works on a [`Transport`] trait object, which is
//! what lets the simulator drive the real protocol code in tests. The trait is
//! deliberately tiny: read, write, flush, and the two modem-control lines. Every
//! method returns a [`Result`] because a pulled cable is a normal event here,
//! not an exceptional one.

use std::io;

use crate::error::{Error, Result};

/// The minimum a serial port has to be able to do for this crate.
///
/// Implemented by [`Port`] for real hardware and by the simulator's fake device
/// for tests. Kept object-safe so a `&mut dyn Transport` can be passed down the
/// stack without generics leaking into every signature.
pub trait Transport: std::fmt::Debug {
    /// Read up to `buf.len()` bytes. Returns the count, which may be zero when
    /// the port times out with nothing to give.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Write all of `buf`. A short write is an error: the protocol has no
    /// partial-message mode, and silently accepting one desynchronises the chip.
    fn write_all(&mut self, buf: &[u8]) -> Result<()>;

    /// Flush whatever the OS has buffered.
    fn flush(&mut self) -> Result<()>;

    /// Drop anything already received. Used before a command so stale bytes from
    /// the previous one cannot be mistaken for this one's reply.
    fn clear_input(&mut self) -> Result<()>;

    /// Set the baud rate. The BootROM is fixed at 115200; the DA can switch.
    fn set_baud(&mut self, baud: u32) -> Result<()>;

    /// Assert or release DTR and RTS together.
    ///
    /// The two lines are handled as one because the protocol never uses them
    /// independently, and because asserting them at a *running* firmware leaves
    /// the device unresponsive until the battery is removed. That danger is why
    /// the callers gate this behind the NMEA probe rather than calling it
    /// directly.
    fn set_control_lines(&mut self, asserted: bool) -> Result<()>;
}

/// A real serial port.
#[derive(Debug)]
pub struct Port {
    inner: Box<dyn serialport::SerialPort>,
    label: String,
}

impl Port {
    /// Open `name` at `baud` with the timeout the protocol needs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Port`] when the device is missing, busy, or the user is
    /// not in the group that may read it. The message keeps the OS's own wording
    /// because "permission denied" and "no such file" need different fixes.
    pub fn open(name: &str, baud: u32) -> Result<Self> {
        let inner = serialport::new(name, baud)
            .timeout(std::time::Duration::from_millis(200))
            .open()
            .map_err(Error::from)?;
        Ok(Self {
            inner,
            label: name.to_owned(),
        })
    }

    /// The port name this was opened with.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// List the serial ports the OS knows about.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Port`] when enumeration itself fails, which on Linux
    /// usually means no udev/`/dev` access at all.
    pub fn list() -> Result<Vec<String>> {
        let ports = serialport::available_ports().map_err(Error::from)?;
        Ok(ports.into_iter().map(|p| p.port_name).collect())
    }
}

impl Transport for Port {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self.inner.read(buf) {
            Ok(count) => Ok(count),
            Err(err) if err.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(err) => Err(Error::from(err)),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.inner.write_all(buf).map_err(Error::from)
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.flush().map_err(Error::from)
    }

    fn clear_input(&mut self) -> Result<()> {
        self.inner
            .clear(serialport::ClearBuffer::Input)
            .map_err(Error::from)
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.inner.set_baud_rate(baud).map_err(Error::from)
    }

    fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
        self.inner
            .write_data_terminal_ready(asserted)
            .map_err(Error::from)?;
        self.inner
            .write_request_to_send(asserted)
            .map_err(Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transport that records calls, so the trait contract can be tested
    /// without a device.
    #[derive(Debug, Default)]
    struct Recorder {
        written: Vec<u8>,
        baud: Vec<u32>,
        lines: Vec<bool>,
    }

    impl Transport for Recorder {
        fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
            Ok(0)
        }
        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            self.written.extend_from_slice(buf);
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
        fn clear_input(&mut self) -> Result<()> {
            Ok(())
        }
        fn set_baud(&mut self, baud: u32) -> Result<()> {
            self.baud.push(baud);
            Ok(())
        }
        fn set_control_lines(&mut self, asserted: bool) -> Result<()> {
            self.lines.push(asserted);
            Ok(())
        }
    }

    #[test]
    fn the_trait_is_object_safe() {
        let mut port: Box<dyn Transport> = Box::new(Recorder::default());
        port.write_all(b"\xa0").expect("write into the recorder");
        port.set_baud(115_200).expect("set baud on the recorder");
        port.set_control_lines(true)
            .expect("assert on the recorder");
    }

    #[test]
    fn a_write_reaches_the_transport_whole() {
        let mut recorder = Recorder::default();
        recorder
            .write_all(&[0xa0, 0x0a, 0x50, 0x05])
            .expect("record the sync probe");
        assert_eq!(recorder.written, vec![0xa0, 0x0a, 0x50, 0x05]);
    }
}

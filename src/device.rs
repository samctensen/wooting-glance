use anyhow::{Context, Result, bail, ensure};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::time::{Duration, Instant};
use wooting_glance::{
    protocol,
    recovery::{Identity, Keyboard},
};

const VENDOR: u16 = 0x31e3;
const CONFIG_USAGE: u16 = 0xff55;

fn supported(info: &DeviceInfo) -> bool {
    info.vendor_id() == VENDOR
        && matches!(info.product_id(), 0x1400..=0x1402)
        && info.usage_page() == CONFIG_USAGE
}

pub fn list() -> Result<()> {
    let api = HidApi::new()?;
    let devices: Vec<_> = api.device_list().filter(|d| supported(d)).collect();
    println!(
        "{} supported Wooting 80HE configuration interface(s)",
        devices.len()
    );
    for device in devices {
        println!(
            "  {} — USB {:04x}:{:04x}",
            device.product_string().unwrap_or("Wooting 80HE"),
            VENDOR,
            device.product_id()
        );
    }
    Ok(())
}

pub struct Device {
    handle: HidDevice,
    pub identity: Identity,
}

impl Device {
    pub fn open() -> Result<Self> {
        let api = HidApi::new().context("cannot initialize macOS HID")?;
        let devices: Vec<_> = api.device_list().filter(|d| supported(d)).collect();
        ensure!(
            devices.len() == 1,
            "expected one supported Wooting 80HE, found {}",
            devices.len()
        );
        let info = devices[0];
        let handle = info
            .open_device(&api)
            .context("cannot open Wooting configuration interface")?;
        ensure!(
            !handle.is_open_exclusive()?,
            "refusing an exclusive HID connection"
        );
        Ok(Self {
            handle,
            identity: Identity {
                product_id: info.product_id(),
                serial: info.serial_number().unwrap_or_default().to_owned(),
            },
        })
    }

    // Only read commands are available here. Output uploads have a separate path.
    fn query(&self, command: u8, slot: u8) -> Result<Vec<u8>> {
        ensure!(
            matches!(command, 11 | 66 | 68),
            "unsupported diagnostic read command"
        );
        let mut buffer = [0u8; 2048];
        for _ in 0..32 {
            if self.handle.read_timeout(&mut buffer, 1)? == 0 {
                break;
            }
        }
        self.handle
            .send_feature_report(&[1, 0xd1, 0xda, command, slot, 0, 0, 0])?;
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            let size = self.handle.read_timeout(&mut buffer, 50)?;
            if let Some(body) = protocol::response(&buffer[..size], command)? {
                return Ok(body.to_vec());
            }
        }
        bail!(
            "no response to command {command}; firmware compatibility or another HID client may be involved"
        )
    }

    pub fn colors(&self) -> Result<Vec<[u8; 3]>> {
        protocol::colors(&self.query(68, 0)?)
    }
}

impl Keyboard for Device {
    fn colors(&self) -> Result<Vec<[u8; 3]>> {
        self.colors()
    }
    fn active_slot(&self) -> Result<u8> {
        let body = self.query(11, 0)?;
        let slot = *body.first().context("empty active-profile reply")?;
        ensure!(
            slot < 4 && body.get(1).is_none_or(|namespace| *namespace == 0),
            "active profile is not a supported onboard slot"
        );
        Ok(slot)
    }

    fn stored_profile(&self, slot: u8) -> Result<Vec<u8>> {
        ensure!(slot < 4, "unsupported profile slot");
        self.query(66, slot)
    }

    fn write_profile(&self, slot: u8, body: &[u8]) -> Result<()> {
        let report = protocol::upload(slot, body)?;
        let count = self.handle.write(&report)?;
        ensure!(
            count == report.len(),
            "incomplete light-bar upload ({count}/{})",
            report.len()
        );
        // Firmware exposes no documented mandatory upload acknowledgement.
        // Allow the render loop to apply the frame before a readback query.
        let deadline = Instant::now() + Duration::from_millis(150);
        let mut buffer = [0u8; 2048];
        while Instant::now() < deadline {
            let size = self.handle.read_timeout(&mut buffer, 10)?;
            // An acknowledgement is not mandatory, but reject an explicit
            // error if this firmware sends one. Ignore background-service traffic.
            protocol::response(&buffer[..size], 32)?;
        }
        Ok(())
    }
}

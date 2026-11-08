//! The network the door is served on: the board's Wi-Fi, or the emulated Ethernet of Espressif's QEMU when the
//! firmware is built with the `qemu` feature. Either waits until it has an address, and lives as long as it is held;
//! its signal is read for the robot's status.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::mac::MAC;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

#[cfg(feature = "qemu")]
pub fn up(
    _modem: Modem<'static>,
    mac: MAC<'static>,
    sysloop: EspSystemEventLoop,
    _nvs: EspDefaultNvsPartition,
) -> anyhow::Result<impl Sized> {
    use esp_idf_svc::eth::{BlockingEth, EspEth, EthDriver};
    let driver = EthDriver::new_openeth(mac, sysloop.clone())?;
    let mut eth = BlockingEth::wrap(EspEth::wrap(driver)?, sysloop)?;
    eth.start()?;
    eth.wait_netif_up()?;
    log::info!("Ethernet up: {:?}", eth.eth().netif().get_ip_info()?);
    Ok(eth)
}

/// The emulated Ethernet has no signal to read.
#[cfg(feature = "qemu")]
pub fn rssi() -> Option<i8> {
    None
}

#[cfg(not(feature = "qemu"))]
pub fn up(
    modem: Modem<'static>,
    _mac: MAC<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> anyhow::Result<impl Sized> {
    use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
    // The network the board joins, set when the firmware is built and never committed.
    let (Some(ssid), Some(password)) = (option_env!("BEST_MCU_WIFI_SSID"), option_env!("BEST_MCU_WIFI_PASSWORD"))
    else {
        anyhow::bail!("no network to join: build with BEST_MCU_WIFI_SSID and BEST_MCU_WIFI_PASSWORD set");
    };
    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sysloop.clone(), Some(nvs))?, sysloop)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("BEST_MCU_WIFI_SSID is longer than 32 bytes"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow::anyhow!("BEST_MCU_WIFI_PASSWORD is longer than 64 bytes"))?,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    }))?;
    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    log::info!("Wi-Fi up: {:?}", wifi.wifi().sta_netif().get_ip_info()?);
    Ok(wifi)
}

/// The signal of the access point the board is joined to, in dBm; none while it is not joined.
#[cfg(not(feature = "qemu"))]
pub fn rssi() -> Option<i8> {
    use esp_idf_svc::sys;
    let mut ap = sys::wifi_ap_record_t::default();
    (unsafe { sys::esp_wifi_sta_get_ap_info(&mut ap) } == sys::ESP_OK).then_some(ap.rssi)
}

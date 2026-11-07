//! The network the door is served on: the board's Wi-Fi, or the emulated Ethernet of Espressif's QEMU when the
//! firmware is built with the `qemu` feature. Either waits until it has an address, and lives as long as it is held.

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;

#[cfg(feature = "qemu")]
pub fn up(peripherals: Peripherals, sysloop: EspSystemEventLoop) -> anyhow::Result<impl Sized> {
    use esp_idf_svc::eth::{BlockingEth, EspEth, EthDriver};
    let driver = EthDriver::new_openeth(peripherals.mac, sysloop.clone())?;
    let mut eth = BlockingEth::wrap(EspEth::wrap(driver)?, sysloop)?;
    eth.start()?;
    eth.wait_netif_up()?;
    log::info!("Ethernet up: {:?}", eth.eth().netif().get_ip_info()?);
    Ok(eth)
}

#[cfg(not(feature = "qemu"))]
pub fn up(peripherals: Peripherals, sysloop: EspSystemEventLoop) -> anyhow::Result<impl Sized> {
    use esp_idf_svc::nvs::EspDefaultNvsPartition;
    use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
    // The network the board joins, set when the firmware is built and never committed.
    let (Some(ssid), Some(password)) = (option_env!("BEST_MCU_WIFI_SSID"), option_env!("BEST_MCU_WIFI_PASSWORD"))
    else {
        anyhow::bail!("no network to join: build with BEST_MCU_WIFI_SSID and BEST_MCU_WIFI_PASSWORD set");
    };
    let nvs = EspDefaultNvsPartition::take()?;
    let mut wifi = BlockingWifi::wrap(EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?, sysloop)?;
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

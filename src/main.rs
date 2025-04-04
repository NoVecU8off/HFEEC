#![allow(dead_code)]
mod cpu;
mod dpdk;
mod numa;
mod packet;
mod protocols;

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::dpdk::config::default_dpdk_config;
use crate::numa::manager::NumaManager;
use crate::packet::data::PacketData;

fn main() {
    println!("Starting HFEEC - High Frequency Electronic Exchange Connector");

    // Создаем менеджер NUMA
    let mut numa_manager = match NumaManager::new() {
        Ok(manager) => manager,
        Err(e) => {
            eprintln!("Failed to initialize NUMA manager: {}", e);
            return;
        }
    };

    // Инициализируем NUMA-узлы
    if let Err(e) = numa_manager.init_nodes() {
        eprintln!("Failed to initialize NUMA nodes: {}", e);
        return;
    }

    // Выводим информацию о топологии
    numa_manager.print_numa_topology();

    // Создаем конфигурацию DPDK
    let mut dpdk_config = default_dpdk_config();

    // Настраиваем DPDK с учетом количества узлов NUMA
    let node_count = numa_manager.get_node_count();
    dpdk_config = dpdk_config.with_numa_allocation(node_count, 1024);

    // Включаем поддержку Jumbo Frames
    dpdk_config = dpdk_config.with_jumbo_frames(9000);

    // Распределяем интерфейсы по узлам NUMA
    if let Err(e) = numa_manager.distribute_interfaces(&dpdk_config) {
        eprintln!("Failed to distribute interfaces: {}", e);
        return;
    }

    // Инициализируем DPDK для всех узлов
    if let Err(e) = numa_manager.init_dpdk(&dpdk_config) {
        eprintln!("Failed to initialize DPDK: {}", e);
        return;
    }

    // Создаем обработчик пакетов
    let packet_handler = Arc::new(|_queue_id: u16, packet: &PacketData| {
        // В реальном коде здесь была бы обработка пакетов
        // Для примера просто считаем количество пакетов
        static mut PACKET_COUNT: u64 = 0;
        static mut LAST_REPORT: u64 = 0;

        unsafe {
            PACKET_COUNT += 1;

            // Выводим статистику каждые 1 000 000 пакетов
            if PACKET_COUNT - LAST_REPORT >= 1_000_000 {
                // Выводим первые несколько байт данных (для отладки)
                let data = packet.get_data();
                if data.len() > 16 {
                    println!("Data sample: {:02X?}", &data[0..16]);
                }

                LAST_REPORT = PACKET_COUNT;
            }
        }
    });

    if let Err(e) = numa_manager.start_packet_processing(packet_handler, &dpdk_config) {
        eprintln!("Failed to start packet processing: {}", e);
        return;
    }

    println!("Packet processing started. Press Ctrl+C to stop.");

    loop {
        thread::sleep(Duration::from_secs(1));
    }

    // numa_manager.stop_packet_processing();
}

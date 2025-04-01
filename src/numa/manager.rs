// src/numa/manager.rs
use std::collections::HashMap;

use crate::cpu::topology::CpuTopology;
use crate::dpdk::config::DpdkConfig;
use crate::dpdk::init::{configure_port_for_node, enumerate_dpdk_ports, init_dpdk_for_node};
use crate::numa::ffi::NumaAllocator;
use crate::numa::node::NumaNode;
use crate::numa::topology::NumaTopology;

/// Управляет созданием и инициализацией изолированных узлов NUMA
pub struct NumaManager {
    /// Информация о топологии CPU
    cpu_topology: CpuTopology,
    /// Информация о топологии NUMA
    numa_topology: NumaTopology,
    /// Карта узлов NUMA
    nodes: HashMap<usize, NumaNode>,
    /// Признак, что NUMA доступна
    numa_available: bool,
}

impl NumaManager {
    /// Создает новый менеджер NUMA
    pub fn new() -> Result<Self, String> {
        // Загружаем информацию о топологии
        let cpu_topology =
            CpuTopology::new().map_err(|e| format!("Failed to load CPU topology: {}", e))?;

        let numa_topology =
            NumaTopology::new().map_err(|e| format!("Failed to load NUMA topology: {}", e))?;

        // Проверяем, доступна ли NUMA
        let numa_available = NumaAllocator::is_available();

        println!("NUMA support available: {}", numa_available);

        Ok(Self {
            cpu_topology,
            numa_topology,
            nodes: HashMap::new(),
            numa_available,
        })
    }

    /// Инициализирует необходимое количество NUMA-узлов
    pub fn init_nodes(&mut self) -> Result<(), String> {
        // Определяем количество узлов NUMA
        let node_count = if self.numa_available {
            NumaAllocator::get_node_count()
        } else {
            1 // Если NUMA недоступна, создаем один "виртуальный" узел
        };

        println!("Initializing {} NUMA nodes", node_count);

        // Создаем узлы
        for node_id in 0..node_count {
            let node = NumaNode::new(node_id, &self.cpu_topology, &self.numa_topology);
            self.nodes.insert(node_id, node);
        }

        // Выводим информацию о созданных узлах
        self.print_numa_topology();

        Ok(())
    }

    /// Распределяет сетевые интерфейсы по NUMA-узлам
    pub fn distribute_interfaces(&mut self, dpdk_config: &DpdkConfig) -> Result<(), String> {
        // Перечисляем доступные порты DPDK
        let ports = enumerate_dpdk_ports();
        if ports.is_empty() {
            return Err("No DPDK ports found".to_string());
        }

        println!("Found {} DPDK ports", ports.len());

        // Распределяем порты по узлам NUMA
        for port in ports {
            // Определяем узел NUMA для этого порта
            let node_id = if let Some(port_node) = port.numa_node {
                // Если порт прикреплен к конкретному узлу NUMA
                port_node
            } else {
                // Если неизвестно, к какому узлу прикреплен порт, используем узел 0
                0
            };

            // Регистрируем порт на соответствующем узле
            if let Some(node) = self.nodes.get_mut(&node_id) {
                node.register_port(
                    port.port_id,
                    &port.if_name,
                    dpdk_config.num_rx_queues,
                    dpdk_config.num_tx_queues,
                    &self.numa_topology,
                );
            } else {
                return Err(format!("NUMA node {} not available", node_id));
            }
        }

        Ok(())
    }

    /// Инициализирует DPDK для всех NUMA-узлов
    pub fn init_dpdk(&mut self, dpdk_config: &DpdkConfig) -> Result<(), String> {
        // Для каждого узла NUMA
        for (node_id, node) in &mut self.nodes {
            // Инициализируем DPDK для этого узла
            let mut node_args = vec![];

            if self.numa_available {
                // Добавляем аргументы для NUMA
                node_args.push(format!("--socket-id={}", node_id));
            }

            println!("Initializing DPDK for NUMA node {}", node_id);

            // Инициализируем EAL
            init_dpdk_for_node(node, dpdk_config, &node_args)?;

            // Конфигурируем порты для этого узла
            for port in &node.local_ports {
                configure_port_for_node(node, port.port_id, dpdk_config)?;
            }

            // Инициализируем пул пакетов для узла с учетом NUMA
            let pool_capacity = (dpdk_config.burst_size * 4) as usize;
            node.init_packet_pool(pool_capacity)?;
        }

        Ok(())
    }

    /// Запускает обработку пакетов на всех узлах NUMA
    pub fn start_packet_processing(
        &mut self,
        packet_handler: crate::numa::node::PacketHandler,
        dpdk_config: &DpdkConfig,
    ) -> Result<(), String> {
        println!("Starting packet processing on all NUMA nodes");

        // Для каждого узла NUMA
        for (node_id, node) in &mut self.nodes {
            println!("Starting workers on NUMA node {}", node_id);

            // Запускаем обработку пакетов
            node.start_workers(packet_handler.clone(), dpdk_config.burst_size)?;
        }

        Ok(())
    }

    /// Останавливает обработку пакетов на всех узлах NUMA
    pub fn stop_packet_processing(&mut self) {
        println!("Stopping packet processing on all NUMA nodes");

        // Для каждого узла NUMA
        for (node_id, node) in &mut self.nodes {
            println!("Stopping workers on NUMA node {}", node_id);
            node.stop_workers();
        }
    }

    /// Выводит информацию о топологии NUMA
    pub fn print_numa_topology(&self) {
        println!("==== NUMA Topology Information ====");
        println!("NUMA available: {}", self.numa_available);
        println!("NUMA nodes: {}", self.nodes.len());

        // Информация о CPU
        self.cpu_topology.print_topology_info();

        // Информация о NUMA
        self.numa_topology.print_topology_info(&self.cpu_topology);

        // Информация о созданных узлах
        for (node_id, node) in &self.nodes {
            println!("\nNUMA Node {}:", node_id);
            println!(
                "  CPU cores: {:?}",
                node.local_cpus.iter().map(|c| c.id).collect::<Vec<_>>()
            );
            println!("  Ports: {}", node.local_ports.len());

            for port in &node.local_ports {
                println!(
                    "    Port {} ({}): RX queues: {}, TX queues: {}",
                    port.port_id, port.if_name, port.num_rx_queues, port.num_tx_queues
                );
            }
        }

        println!("====================================");
    }

    /// Проверяет, доступна ли NUMA
    pub fn is_numa_available(&self) -> bool {
        self.numa_available
    }

    /// Возвращает количество узлов NUMA
    pub fn get_node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Возвращает ссылку на конкретный узел NUMA
    pub fn get_node(&self, node_id: usize) -> Option<&NumaNode> {
        self.nodes.get(&node_id)
    }

    /// Возвращает мутабельную ссылку на конкретный узел NUMA
    pub fn get_node_mut(&mut self, node_id: usize) -> Option<&mut NumaNode> {
        self.nodes.get_mut(&node_id)
    }
}

impl Drop for NumaManager {
    fn drop(&mut self) {
        self.stop_packet_processing();
    }
}

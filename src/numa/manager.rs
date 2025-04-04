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
        let cpu_topology =
            CpuTopology::new().map_err(|e| format!("Failed to load CPU topology: {}", e))?;

        let numa_topology =
            NumaTopology::new().map_err(|e| format!("Failed to load NUMA topology: {}", e))?;

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
        let node_count = if self.numa_available {
            NumaAllocator::get_node_count()
        } else {
            1
        };

        println!("Initializing {} NUMA nodes", node_count);

        for node_id in 0..node_count {
            let node = NumaNode::new(node_id, &self.cpu_topology, &self.numa_topology);
            self.nodes.insert(node_id, node);
        }

        self.print_numa_topology();

        Ok(())
    }

    /// Распределяет сетевые интерфейсы по NUMA-узлам
    pub fn distribute_interfaces(&mut self, dpdk_config: &DpdkConfig) -> Result<(), String> {
        let ports = enumerate_dpdk_ports();
        if ports.is_empty() {
            return Err("No DPDK ports found".to_string());
        }

        println!("Found {} DPDK ports", ports.len());

        for port in ports {
            let node_id = port.numa_node.unwrap_or_default();

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
        for (node_id, node) in &mut self.nodes {
            let mut node_args = vec![];

            if self.numa_available {
                node_args.push(format!("--socket-id={}", node_id));
            }

            println!("Initializing DPDK for NUMA node {}", node_id);

            init_dpdk_for_node(node, dpdk_config, &node_args)?;

            for port in &node.local_ports {
                configure_port_for_node(node, port.port_id, dpdk_config)?;
            }
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

        for (node_id, node) in &mut self.nodes {
            println!("Starting workers on NUMA node {}", node_id);

            node.start_workers(packet_handler.clone(), dpdk_config.burst_size)?;
        }

        Ok(())
    }

    /// Останавливает обработку пакетов на всех узлах NUMA
    pub fn stop_packet_processing(&mut self) {
        println!("Stopping packet processing on all NUMA nodes");

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

        self.cpu_topology.print_topology_info();

        self.numa_topology.print_topology_info(&self.cpu_topology);

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

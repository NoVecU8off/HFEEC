// src/cpu/worker.rs - Worker thread management with CPU and NUMA affinity
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};

use core_affinity::CoreId;

use super::numa::NumaTopology;
use super::topology::CpuTopology;
use crate::dpdk::packet::PacketData;
use crate::dpdk::pool::PacketDataPool;

/// The type of callback function used for packet processing
pub type PacketHandler = Arc<dyn Fn(u16, &PacketData) + Send + Sync + 'static>;

/// Information about a DPDK port/NIC
#[derive(Debug)]
pub struct PortInfo {
    /// DPDK port ID
    pub port_id: u16,
    /// The name of the network interface
    pub if_name: String,
    /// NUMA node this port belongs to
    pub numa_node: Option<usize>,
    /// Number of RX queues allocated for this port
    pub num_rx_queues: u16,
    /// Number of TX queues allocated for this port
    pub num_tx_queues: u16,
}

/// Worker thread information
#[derive(Debug)]
pub struct Worker {
    /// Thread handle
    pub thread: Option<JoinHandle<()>>,
    /// Core ID this thread is pinned to
    pub core_id: CoreId,
    /// DPDK port this worker processes
    pub port_id: u16,
    /// DPDK RX queue this worker processes
    pub queue_id: u16,
    /// NUMA node this worker is running on
    pub numa_node: Option<usize>,
}

/// Manager for worker threads with NUMA-awareness
#[derive(Debug)]
pub struct WorkerManager {
    /// CPU topology information
    cpu_topology: Arc<CpuTopology>,
    /// NUMA topology information
    numa_topology: Arc<NumaTopology>,
    /// Running flag shared with workers
    running: Arc<AtomicBool>,
    /// List of worker threads
    workers: Vec<Worker>,
    /// Information about DPDK ports
    ports: HashMap<u16, PortInfo>,
}

impl WorkerManager {
    /// Creates a new worker manager
    pub fn new(cpu_topology: Arc<CpuTopology>, numa_topology: Arc<NumaTopology>) -> Self {
        WorkerManager {
            cpu_topology,
            numa_topology,
            running: Arc::new(AtomicBool::new(false)),
            workers: Vec::new(),
            ports: HashMap::new(),
        }
    }

    /// Registers a DPDK port with the worker manager
    pub fn register_port(
        &mut self,
        port_id: u16,
        if_name: &str,
        num_rx_queues: u16,
        num_tx_queues: u16,
    ) {
        let numa_node = self.numa_topology.get_nic_node(if_name);

        println!("Registering port {} ({})", port_id, if_name);
        println!("  NUMA node: {:?}", numa_node);
        println!("  RX queues: {}", num_rx_queues);
        println!("  TX queues: {}", num_tx_queues);

        self.ports.insert(
            port_id,
            PortInfo {
                port_id,
                if_name: if_name.to_string(),
                numa_node,
                num_rx_queues,
                num_tx_queues,
            },
        );
    }

    /// Starts worker threads with CPU affinity based on NUMA topology
    pub fn start_workers(
        &mut self,
        packet_handler: PacketHandler,
        burst_size: u32,
    ) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) {
            return Err("Workers already running".to_string());
        }

        self.running.store(true, Ordering::SeqCst);

        // For each registered port
        for port_info in self.ports.values() {
            let port_id = port_info.port_id;
            let numa_node = port_info.numa_node;
            let num_rx_queues = port_info.num_rx_queues;

            let packet_pool = Arc::new(PacketDataPool::new(burst_size as usize * 4));

            // Get cores to use for this port based on NUMA node
            let cores = match numa_node {
                Some(node) => {
                    let node_cores = self
                        .numa_topology
                        .get_node_core_ids(node, &self.cpu_topology);
                    if node_cores.is_empty() {
                        println!(
                            "Warning: No cores found for NUMA node {}, using all available cores",
                            node
                        );
                        self.cpu_topology.get_filtered_core_ids()
                    } else {
                        node_cores
                    }
                }
                None => self.cpu_topology.get_filtered_core_ids(),
            };

            println!(
                "Starting {} worker threads for port {} on NUMA node {:?}",
                num_rx_queues, port_id, numa_node
            );
            println!(
                "Available cores: {:?}",
                cores.iter().map(|c| c.id).collect::<Vec<_>>()
            );

            if cores.is_empty() {
                return Err(format!("No cores available for port {}", port_id));
            }

            // Create one worker thread per RX queue
            for queue_id in 0..num_rx_queues {
                // Round-robin core assignment if we have more queues than cores
                let core_idx = (queue_id as usize) % cores.len();
                let core_id = cores[core_idx];

                println!("  Queue {} -> Core {}", queue_id, core_id.id);

                // Start worker thread for this queue
                let worker = self.start_worker_thread(
                    port_id,
                    queue_id,
                    core_id,
                    numa_node,
                    packet_handler.clone(),
                    packet_pool.clone(),
                    burst_size,
                );

                self.workers.push(worker);
            }
        }

        println!("Started {} worker threads", self.workers.len());
        Ok(())
    }

    /// Starts a single worker thread
    fn start_worker_thread(
        &self,
        port_id: u16,
        queue_id: u16,
        core_id: CoreId,
        numa_node: Option<usize>,
        packet_handler: PacketHandler,
        packet_pool: Arc<PacketDataPool>,
        burst_size: u32,
    ) -> Worker {
        let running = self.running.clone();

        // Create a thread with the specified core affinity
        let thread = thread::spawn(move || {
            // Set thread affinity
            core_affinity::set_for_current(core_id);

            // DPDK rx_burst buffer
            let mut rx_pkts = vec![std::ptr::null_mut(); burst_size as usize];

            // Main processing loop
            while running.load(Ordering::SeqCst) {
                // This unsafe code would be replaced with the actual DPDK rx_burst call
                // and packet processing logic - we're just showing the structure here
                let nb_rx = unsafe {
                    crate::dpdk::ffi::rte_eth_rx_burst(
                        port_id,
                        queue_id,
                        rx_pkts.as_mut_ptr(),
                        burst_size as u16,
                    )
                };

                for i in 0..nb_rx as usize {
                    let pkt = rx_pkts[i];

                    // Extract packet data
                    let mut src_ip_ptr = std::ptr::null_mut();
                    let mut src_ip_len: u32 = 0;
                    let mut dst_ip_ptr = std::ptr::null_mut();
                    let mut dst_ip_len: u32 = 0;
                    let mut src_port: u16 = 0;
                    let mut dst_port: u16 = 0;
                    let mut data_ptr = std::ptr::null_mut();
                    let mut data_len: u32 = 0;

                    let ret = unsafe {
                        crate::dpdk::ffi::dpdk_extract_packet_data(
                            pkt,
                            &mut src_ip_ptr,
                            &mut src_ip_len,
                            &mut dst_ip_ptr,
                            &mut dst_ip_len,
                            &mut src_port,
                            &mut dst_port,
                            &mut data_ptr,
                            &mut data_len,
                        )
                    };

                    if ret == 0 && !data_ptr.is_null() && data_len > 0 {
                        let mut packet = packet_pool.acquire();

                        packet.source_port = src_port;
                        packet.dest_port = dst_port;
                        packet.queue_id = queue_id;

                        packet.source_ip_ptr = src_ip_ptr;
                        packet.source_ip_len = src_ip_len as usize;
                        packet.dest_ip_ptr = dst_ip_ptr;
                        packet.dest_ip_len = dst_ip_len as usize;
                        packet.data_ptr = data_ptr;
                        packet.data_len = data_len as usize;

                        packet.mbuf_ptr = pkt;

                        // Process packet with user-provided handler
                        packet_handler(queue_id, &packet);

                        // Free the packet buffer
                        unsafe { crate::dpdk::ffi::rte_pktmbuf_free(packet.mbuf_ptr) };
                        packet_pool.release(packet);
                    } else {
                        // Free the packet in case of extraction error
                        unsafe { crate::dpdk::ffi::rte_pktmbuf_free(pkt) };
                    }
                }
            }
        });

        Worker {
            thread: Some(thread),
            core_id,
            port_id,
            queue_id,
            numa_node,
        }
    }

    /// Stops all worker threads
    pub fn stop_workers(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        println!("Stopping {} worker threads", self.workers.len());

        // Signal all threads to stop
        self.running.store(false, Ordering::SeqCst);

        // Wait for all threads to finish
        while let Some(mut worker) = self.workers.pop() {
            if let Some(thread) = worker.thread.take() {
                let _ = thread.join();
                println!(
                    "  Worker thread for port {}, queue {} on core {} stopped",
                    worker.port_id, worker.queue_id, worker.core_id.id
                );
            }
        }
    }

    /// Generates DPDK EAL arguments based on CPU and NUMA topology
    pub fn generate_dpdk_eal_args(
        &self,
        use_huge_pages: bool,
        memory_mb_per_socket: usize,
    ) -> Vec<String> {
        let mut args = Vec::new();

        // Core mask for DPDK threads
        let core_mask = self.cpu_topology.generate_core_mask();
        args.push(format!("--lcores={}", core_mask));

        // Set master lcore to core 0
        args.push("--master-lcore=0".to_string());

        // Configure memory if using huge pages
        if use_huge_pages {
            args.push("--in-memory".to_string());

            // Socket memory configuration based on NUMA topology
            args.extend(
                self.numa_topology
                    .get_socket_memory_config(memory_mb_per_socket),
            );

            args.push("--huge-unlink".to_string());
        }

        args
    }

    /// Returns the optimal NUMA node for a given DPDK port
    pub fn get_port_numa_node(&self, port_id: u16) -> Option<usize> {
        self.ports.get(&port_id).and_then(|info| info.numa_node)
    }

    /// Prints information about worker allocation
    pub fn print_worker_info(&self) {
        println!("Worker Thread Allocation:");

        for port_id in self.ports.keys() {
            let port_workers: Vec<&Worker> = self
                .workers
                .iter()
                .filter(|w| w.port_id == *port_id)
                .collect();

            println!("Port {}: {} worker threads", port_id, port_workers.len());

            for worker in port_workers {
                println!(
                    "  Queue {} -> Core {} (NUMA node {:?})",
                    worker.queue_id, worker.core_id.id, worker.numa_node
                );
            }
        }

        // Check for NUMA-related mismatches
        for worker in &self.workers {
            if let Some(port_info) = self.ports.get(&worker.port_id) {
                if port_info.numa_node != worker.numa_node && port_info.numa_node.is_some() {
                    println!("WARNING: NUMA mismatch detected!");
                    println!(
                        "  Worker processing port {} queue {} is on NUMA node {:?}",
                        worker.port_id, worker.queue_id, worker.numa_node
                    );
                    println!("  But the port is on NUMA node {:?}", port_info.numa_node);
                }
            }
        }
    }
}

impl Drop for WorkerManager {
    fn drop(&mut self) {
        self.stop_workers();
    }
}

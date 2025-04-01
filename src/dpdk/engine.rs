use std::ffi::{c_void, CString};
use std::os::raw::{c_char, c_int, c_ushort};
use std::ptr;
use std::sync::Arc;

use super::ffi::{DpdkConfig, DpdkError, RteEthConf, RteEthRssConf, RteMempool};
use super::hugepages;
use crate::system::cpu::CpuTopology;
use crate::system::numa::NumaTopology;
use crate::system::worker::{PacketHandler, WorkerManager};

/// Enhanced wrapper for DPDK with NUMA and CPU topology awareness
pub struct DpdkApp {
    /// Worker thread manager
    worker_manager: WorkerManager,
    /// CPU topology information
    cpu_topology: Arc<CpuTopology>,
    /// NUMA topology information
    numa_topology: Arc<NumaTopology>,
    /// DPDK configuration
    config: DpdkConfig,
    /// Memory pools for each NUMA node
    mbuf_pools: Vec<(usize, *mut RteMempool)>, // (numa_node, pool_ptr)
    /// Initialization state
    initialized: bool,
}

impl DpdkApp {
    /// Creates a new DPDK application instance
    pub fn new(config: DpdkConfig) -> Result<Self, String> {
        // Load CPU and NUMA topology information
        let cpu_topology = match CpuTopology::new() {
            Ok(t) => Arc::new(t),
            Err(e) => return Err(format!("Failed to load CPU topology: {}", e)),
        };

        let numa_topology = match NumaTopology::new() {
            Ok(t) => Arc::new(t),
            Err(e) => return Err(format!("Failed to load NUMA topology: {}", e)),
        };

        // Create worker manager
        let worker_manager = WorkerManager::new(cpu_topology.clone(), numa_topology.clone());

        Ok(DpdkApp {
            worker_manager,
            cpu_topology,
            numa_topology,
            config,
            mbuf_pools: Vec::new(),
            initialized: false,
        })
    }

    /// Initializes DPDK with topology awareness
    pub fn init(&mut self, port_name: &str, additional_args: &[String]) -> Result<(), DpdkError> {
        if self.initialized {
            return Ok(());
        }

        // Verify hugepages if needed
        if self.config.use_huge_pages && !hugepages::check_hugepages_available() {
            return Err(DpdkError::InitError);
        }

        // Generate EAL arguments based on CPU and NUMA topology
        let mut eal_args = self.worker_manager.generate_dpdk_eal_args(
            self.config.use_huge_pages,
            self.config
                .socket_mem
                .as_ref()
                .map_or(1024, |v| v[0] as usize),
        );

        // Add user-provided arguments
        eal_args.extend_from_slice(additional_args);

        println!("Initializing DPDK with arguments:");
        for arg in &eal_args {
            println!("  {}", arg);
        }

        // Convert arguments to C format
        let c_args: Vec<CString> = eal_args
            .iter()
            .map(|arg| CString::new(arg.as_str()).unwrap())
            .collect();

        let mut c_argv: Vec<*mut c_char> = c_args
            .iter()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();

        // Initialize EAL
        let ret = unsafe { super::ffi::rte_eal_init(c_args.len() as c_int, c_argv.as_mut_ptr()) };
        if ret < 0 {
            return Err(DpdkError::InitError);
        }

        // Register the port with the worker manager
        let port_id = self.config.port_id;
        let num_rx_queues = self.config.num_rx_queues;
        let num_tx_queues = self.config.num_tx_queues;

        self.worker_manager
            .register_port(port_id, port_name, num_rx_queues, num_tx_queues);

        // Create memory pools for each NUMA node
        self.create_mbuf_pools()?;

        self.initialized = true;
        Ok(())
    }

    /// Creates memory pools for each NUMA node
    fn create_mbuf_pools(&mut self) -> Result<(), DpdkError> {
        // Get port's NUMA node
        let port_id = self.config.port_id;
        let port_numa_node = unsafe {
            let node = super::ffi::rte_eth_dev_socket_id(port_id);
            if node >= 0 {
                Some(node as usize)
            } else {
                None
            }
        };

        println!(
            "Creating memory pools for port {} on NUMA node {:?}",
            port_id, port_numa_node
        );

        if let Some(node) = port_numa_node {
            // Create a memory pool specifically for this port's NUMA node
            let pool_name = CString::new(format!("mbuf_pool_node{}", node)).unwrap();
            let mbuf_pool = unsafe {
                super::ffi::rte_pktmbuf_pool_create(
                    pool_name.as_ptr(),
                    self.config.num_mbufs,
                    self.config.mbuf_cache_size,
                    0,
                    self.config.data_room_size,
                    node as c_int,
                )
            };

            if mbuf_pool.is_null() {
                return Err(DpdkError::MemoryError);
            }

            self.mbuf_pools.push((node, mbuf_pool));
        } else {
            // Create a default pool if NUMA information is not available
            let pool_name = CString::new("mbuf_pool_default").unwrap();
            let mbuf_pool = unsafe {
                super::ffi::rte_pktmbuf_pool_create(
                    pool_name.as_ptr(),
                    self.config.num_mbufs,
                    self.config.mbuf_cache_size,
                    0,
                    self.config.data_room_size,
                    -1, // Default socket
                )
            };

            if mbuf_pool.is_null() {
                return Err(DpdkError::MemoryError);
            }

            self.mbuf_pools.push((0, mbuf_pool));
        }

        Ok(())
    }

    /// Configures a DPDK port with queue-to-core mapping
    pub fn configure_port(&self) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        let port_id = self.config.port_id;

        // Check if port is valid
        let is_valid = unsafe { super::ffi::rte_eth_dev_is_valid_port(port_id) };
        if is_valid == 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Get port's NUMA node
        let port_socket_id = unsafe {
            let socket_id = super::ffi::rte_eth_dev_socket_id(port_id);
            if socket_id >= 0 {
                socket_id
            } else {
                -1
            }
        };

        println!("Configuring port {} on socket {}", port_id, port_socket_id);

        // Get the memory pool for this socket
        let mbuf_pool = self.get_mbuf_pool_for_socket(port_socket_id as usize);
        if mbuf_pool.is_null() {
            return Err(DpdkError::MemoryError);
        }

        // Create the port configuration
        let mut eth_conf = default_eth_config();

        // Configure RSS if needed
        let enable_rss = self.config.enable_rss && self.config.num_rx_queues > 1;
        if enable_rss {
            eth_conf.rxmode.mq_mode = super::ffi::ETH_MQ_RX_RSS;
            eth_conf.rx_adv_conf.rss_conf.rss_hf = self.config.rss_hf;

            if let Some(ref key) = self.config.rss_key {
                eth_conf.rx_adv_conf.rss_conf.rss_key = key.as_ptr() as *mut u8;
                eth_conf.rx_adv_conf.rss_conf.rss_key_len = key.len() as u8;
            }
        }

        // Configure the port
        let ret = unsafe {
            super::ffi::rte_eth_dev_configure(
                port_id,
                self.config.num_rx_queues,
                self.config.num_tx_queues,
                &eth_conf as *const RteEthConf as *const c_void,
            )
        };

        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Set up RX queues
        for q in 0..self.config.num_rx_queues {
            // Calculate the socket ID to use for this queue based on the core it will run on
            let queue_socket_id = match self.config.use_numa_on_socket {
                true => port_socket_id, // Use the port's socket
                false => -1,            // Use default socket
            };

            let ret = unsafe {
                super::ffi::rte_eth_rx_queue_setup(
                    port_id,
                    q,
                    self.config.rx_ring_size as c_ushort,
                    queue_socket_id,
                    ptr::null(),
                    mbuf_pool,
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        // Set up TX queues
        for q in 0..self.config.num_tx_queues {
            let queue_socket_id = match self.config.use_numa_on_socket {
                true => port_socket_id, // Use the port's socket
                false => -1,            // Use default socket
            };

            let ret = unsafe {
                super::ffi::rte_eth_tx_queue_setup(
                    port_id,
                    q,
                    self.config.tx_ring_size as c_ushort,
                    queue_socket_id,
                    ptr::null(),
                )
            };

            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        // Start the port
        let ret = unsafe { super::ffi::rte_eth_dev_start(port_id) };
        if ret < 0 {
            return Err(DpdkError::PortConfigError);
        }

        // Enable promiscuous mode if requested
        if self.config.promiscuous {
            let ret = unsafe { super::ffi::rte_eth_promiscuous_enable(port_id) };
            if ret < 0 {
                return Err(DpdkError::PortConfigError);
            }
        }

        Ok(())
    }

    /// Helper to get the memory pool for a specific socket
    fn get_mbuf_pool_for_socket(&self, socket_id: usize) -> *mut RteMempool {
        // Try to find a pool for the specified socket
        for &(node, pool) in &self.mbuf_pools {
            if node == socket_id {
                return pool;
            }
        }

        // If not found, return the first available pool
        if !self.mbuf_pools.is_empty() {
            return self.mbuf_pools[0].1;
        }

        // No pools available
        ptr::null_mut()
    }

    /// Starts packet processing with worker threads
    pub fn start_processing(&mut self, packet_handler: PacketHandler) -> Result<(), DpdkError> {
        if !self.initialized {
            return Err(DpdkError::NotInitialized);
        }

        // Start worker threads
        match self
            .worker_manager
            .start_workers(packet_handler, self.config.burst_size)
        {
            Ok(_) => {
                // Print allocation information
                self.worker_manager.print_worker_info();
                Ok(())
            }
            Err(e) => {
                eprintln!("Failed to start workers: {}", e);
                Err(DpdkError::RunningError)
            }
        }
    }

    /// Stops processing and worker threads
    pub fn stop(&mut self) {
        if !self.initialized {
            return;
        }

        // Stop worker threads
        self.worker_manager.stop_workers();

        // Stop the port
        unsafe {
            super::ffi::rte_eth_dev_stop(self.config.port_id);
        }
    }

    /// Completely cleans up DPDK resources
    pub fn cleanup(&mut self) {
        if !self.initialized {
            return;
        }

        self.stop();

        // Close the port
        unsafe {
            super::ffi::rte_eth_dev_close(self.config.port_id);
        }

        // Clean up EAL
        unsafe {
            super::ffi::rte_eal_cleanup();
        }

        self.initialized = false;
    }

    /// Prints detailed information about CPU and NUMA topology
    pub fn print_topology_info(&self) {
        println!("==== System Topology Information ====");

        // Print CPU topology
        self.cpu_topology.print_topology_info();

        // Print NUMA topology
        self.numa_topology.print_topology_info(&self.cpu_topology);

        println!("====================================");
    }
}

impl Drop for DpdkApp {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Creates a default Ethernet port configuration
fn default_eth_config() -> RteEthConf {
    RteEthConf {
        rxmode: super::ffi::RteEthRxMode {
            mq_mode: 0,
            max_rx_pkt_len: 0,
            split_hdr_size: 0,
            offloads: 0,
        },
        txmode: super::ffi::RteEthTxMode {
            mq_mode: 0,
            pvid: 0,
            offloads: 0,
        },
        lpbk_mode: 0,
        rx_adv_conf: super::ffi::RteEthRxAdvConf {
            rss_conf: RteEthRssConf {
                rss_key: ptr::null_mut(),
                rss_key_len: 0,
                rss_hf: 0,
            },
        },
        tx_adv_conf: super::ffi::RteEthTxAdvConf {},
        dcb_capability_en: 0,
        fdir_conf: super::ffi::RteEthFdirConf {},
        intr_conf: super::ffi::RteEthIntrConf {},
    }
}

use crate::actor::{self, Actor, Cap};
use crate::cell::CellEvent;
use crate::frame::Payload;
use crate::port::{PortEvent, PortState};
use crate::pollster::{Pollster, PollsterEvent};

#[derive(Debug, Clone)]
pub enum HubEvent {
    Init(Cap<HubEvent>),
    PortStatus(Cap<PortEvent>, PortState),
    PortToHubWrite(Cap<PortEvent>, Payload),
    PortToHubRead(Cap<PortEvent>),
    CellToHubWrite(Cap<CellEvent>, Payload),
    CellToHubRead(Cap<CellEvent>),
}
impl HubEvent {
    pub fn new_init(hub: &Cap<HubEvent>) -> HubEvent {
        HubEvent::Init(hub.clone())
    }
    pub fn new_port_status(port: &Cap<PortEvent>, state: &PortState) -> HubEvent {
        HubEvent::PortStatus(port.clone(), state.clone())
    }
    pub fn new_port_to_hub_write(port: &Cap<PortEvent>, payload: &Payload) -> HubEvent {
        HubEvent::PortToHubWrite(port.clone(), payload.clone())
    }
    pub fn new_port_to_hub_read(port: &Cap<PortEvent>) -> HubEvent {
        HubEvent::PortToHubRead(port.clone())
    }
    pub fn new_cell_to_hub_write(cell: &Cap<CellEvent>, payload: &Payload) -> HubEvent {
        HubEvent::CellToHubWrite(cell.clone(), payload.clone())
    }
    pub fn new_cell_to_hub_read(cell: &Cap<CellEvent>) -> HubEvent {
        HubEvent::CellToHubRead(cell.clone())
    }
}

const MAX_PORTS: usize = 3;

enum Route {
    Cell,
    Port(usize),
}

struct CellIn {
    // Inbound to Cell
    reader: Option<Cap<CellEvent>>,
}

struct CellOut {
    // Outbound from Cell
    writer: Option<Cap<CellEvent>>,
    payload: Option<Payload>,
    send_to: Vec<Route>,
}

struct PortIn {
    // Inbound from port
    writer: Option<Cap<PortEvent>>,
    payload: Option<Payload>,
    send_to: Vec<Route>,
}

struct PortOut {
    // Outbound to port
    reader: Option<Cap<PortEvent>>,
}

// Multi-Port Hub (Node)
pub struct Hub {
    myself: Option<Cap<HubEvent>>,
    ports: Vec<Cap<PortEvent>>,
    cell_in: CellIn,
    cell_out: CellOut,
    port_in: Vec<PortIn>,
    port_out: Vec<PortOut>,
}
impl Hub {
    pub fn create(port_set: &[Cap<PortEvent>]) -> Cap<HubEvent> {
        let ports: Vec<_> = port_set.iter().map(|port| port.clone()).collect();
        let cell_in = CellIn { reader: None };
        let cell_out = CellOut {
            writer: None,
            payload: None,
            send_to: Vec::with_capacity(MAX_PORTS),
        };
        let port_in: Vec<_> = port_set
            .iter()
            .map(|_port| PortIn {
                writer: None,
                payload: None,
                send_to: Vec::with_capacity(MAX_PORTS),
            })
            .collect();
        let port_out: Vec<_> = port_set
            .iter()
            .map(|port| PortOut {
                reader: Some(port.clone()),
            })
            .collect();
        assert_eq!(ports.len(), port_in.len());
        assert_eq!(ports.len(), port_out.len());
        let hub = actor::create(Hub {
            myself: None,
            ports: ports.clone(),
            cell_in,
            cell_out,
            port_in,
            port_out,
        });
        hub.send(HubEvent::new_init(&hub));
        for port in port_set {
            port.send(PortEvent::new_hub_to_port_read(&hub)); // Port ready to receive
        }
        let pollster = Pollster::create(&ports); // create link-failure detector
        pollster.send(PollsterEvent::new_start(&hub));
        // periodically poll ports for liveness
        let cust = hub.clone(); // local copy moved into closure
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(core::time::Duration::from_millis(500));
                pollster.send(PollsterEvent::new_poll(&cust));
            }
        });
        // return Hub capability
        hub
    }
}
impl Actor for Hub {
    type Event = HubEvent;

    fn on_event(&mut self, event: Self::Event) {
        match &event {
            HubEvent::Init(myself) => match &self.myself {
                None => self.myself = Some(myself.clone()),
                Some(_) => panic!("Hub::myself already set"),
            },
            HubEvent::PortStatus(cust, state) => {
                let n = self.port_to_port_num(&cust);
                println!(
                    "Hub::LinkStatus[{}] link_state={:?}, ait_balance={}",
                    n, state.link_state, state.ait_balance
                );
            }
            HubEvent::PortToHubWrite(cust, payload) => {
                println!("Hub::PortToHubWrite");
                let n = self.port_to_port_num(&cust);
                let port_in = &mut self.port_in[n];
                match &port_in.writer {
                    None => {
                        port_in.writer = Some(cust.clone());
                        port_in.payload = Some(payload.clone());
                        self.find_routes(Route::Port(n), &payload);
                        self.try_everyone();
                    }
                    Some(_cust) => panic!("Only one Port-to-Hub writer allowed"),
                }
            }
            HubEvent::PortToHubRead(cust) => {
                println!("Hub::PortToHubRead");
                let n = self.port_to_port_num(&cust);
                let port_out = &mut self.port_out[n];
                match &port_out.reader {
                    None => {
                        port_out.reader = Some(cust.clone());
                        self.try_everyone();
                    }
                    Some(_cust) => panic!("Only one Port-to-Hub reader allowed"),
                }
            }
            HubEvent::CellToHubWrite(cust, payload) => {
                println!("Hub::CellToHubWrite");
                match &self.cell_out.writer {
                    None => {
                        self.cell_out.writer = Some(cust.clone());
                        self.cell_out.payload = Some(payload.clone());
                        self.find_routes(Route::Cell, &payload);
                        self.try_everyone();
                    }
                    Some(_cust) => panic!("Only one Cell-to-Hub writer allowed"),
                }
            }
            HubEvent::CellToHubRead(cust) => {
                println!("Hub::CellToHubRead");
                match &self.cell_in.reader {
                    None => {
                        self.cell_in.reader = Some(cust.clone());
                        self.try_everyone();
                    }
                    Some(_cust) => panic!("Only one Cell-to-Hub reader allowed"),
                }
            }
        }
    }
}
impl Hub {
    fn port_to_port_num(&mut self, port: &Cap<PortEvent>) -> usize {
        self.ports
            .iter()
            .enumerate()
            .find(|(_port_num, port_cap)| *port_cap == port)
            .expect("unknown Port")
            .0
    }
    fn find_routes(&mut self, from: Route, payload: &Payload) {
        // FIXME: this is a completely bogus "routing table" lookup!
        // The TreeId in the Payload should determine the routes, excluding `from`.
        let _tree_id = &payload.id;
        match from {
            Route::Cell => {
                let routes = &mut self.cell_out.send_to;
                assert!(routes.is_empty()); // there shouldn't be any left-over routes
                routes.push(Route::Port(0)); // all Cell tokens route to Port(0)
            }
            Route::Port(n) => {
                let routes = &mut self.port_in[n].send_to;
                assert!(routes.is_empty()); // there shouldn't be any left-over routes
                routes.push(Route::Cell); // all Port(_) tokens route to Cell
            }
        }
    }
    fn send_to_routes(
        hub: &Cap<HubEvent>,
        payload: &Payload,
        routes: &mut Vec<Route>,
        cell_in: &mut CellIn,
        port_out: &mut Vec<PortOut>,
    ) {
        let mut i: usize = 0; // current route index
        while i < routes.len() {
            match routes[i] {
                Route::Cell => {
                    if let Some(cell) = &cell_in.reader {
                        cell.send(CellEvent::new_hub_to_cell_write(&payload));
                        cell_in.reader = None;
                        routes.remove(i);
                    } else {
                        i += 1; // route not ready
                    }
                }
                Route::Port(to) => {
                    if let Some(port) = &port_out[to].reader {
                        port.send(PortEvent::new_hub_to_port_write(&hub, &payload));
                        port_out[to].reader = None;
                        routes.remove(i);
                    } else {
                        i += 1; // route not ready
                    }
                }
            }
        }
    }
    fn try_everyone(&mut self) {
        if let Some(myself) = &self.myself {
            // try sending from Cell
            let cell_out = &mut self.cell_out;
            if let Some(cell) = &cell_out.writer {
                if let Some(payload) = &cell_out.payload {
                    let routes = &mut cell_out.send_to;
                    if !routes.is_empty() {
                        Self::send_to_routes(
                            &myself,
                            &payload,
                            routes,
                            &mut self.cell_in,
                            &mut self.port_out,
                        );
                    } else {
                        // no more routes
                        cell.send(CellEvent::new_hub_to_cell_read()); // ack writer
                        cell_out.writer = None;
                        cell_out.payload = None;
                    }
                }
            }
            // try sending from each Port
            let mut from: usize = 0; // current port number
            while from < self.ports.len() {
                let port_in = &mut self.port_in[from];
                if let Some(port) = &port_in.writer {
                    if let Some(payload) = &port_in.payload {
                        let routes = &mut port_in.send_to;
                        if !routes.is_empty() {
                            Self::send_to_routes(
                                &myself,
                                &payload,
                                routes,
                                &mut self.cell_in,
                                &mut self.port_out,
                            );
                        } else {
                            // no more routes
                            port.send(PortEvent::new_hub_to_port_read(&myself)); // ack writer
                            port_in.writer = None;
                            port_in.payload = None;
                        }
                    }
                }
                from += 1; // next port
            }
        }
    }
}

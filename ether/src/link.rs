use crate::actor::{self, Actor, Cap};
use crate::frame::{self, Frame, Payload, TreeId};
use crate::port::PortEvent;
use crate::wire::WireEvent;
use rand::Rng;

#[derive(Debug, Clone)]
pub enum LinkEvent {
    Frame(Frame),                   // inbound frame received
    Start(Cap<PortEvent>),          // start link activity
    Poll(Cap<PortEvent>),           // link status check
    Stop(Cap<PortEvent>),           // stop link activity
    Read(Cap<PortEvent>),           // reader ready
    Write(Cap<PortEvent>, Payload), // writer full
}
impl LinkEvent {
    pub fn new_frame(frame: &Frame) -> LinkEvent {
        LinkEvent::Frame(frame.clone())
    }
    pub fn new_start(port: &Cap<PortEvent>) -> LinkEvent {
        LinkEvent::Start(port.clone())
    }
    pub fn new_poll(port: &Cap<PortEvent>) -> LinkEvent {
        LinkEvent::Poll(port.clone())
    }
    pub fn new_stop(port: &Cap<PortEvent>) -> LinkEvent {
        LinkEvent::Stop(port.clone())
    }
    pub fn new_read(port: &Cap<PortEvent>) -> LinkEvent {
        LinkEvent::Read(port.clone())
    }
    pub fn new_write(port: &Cap<PortEvent>, payload: &Payload) -> LinkEvent {
        LinkEvent::Write(port.clone(), payload.clone())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    Stop, // link is disabled
    Init, // ready to become entangled
    Run,  // entangled, but quiet
    Live, // entangled with recent activity
}

pub struct Link {
    wire: Cap<WireEvent>,
    nonce: u32,
    state: LinkState,
    balance: isize,
    reader: Option<Cap<PortEvent>>,
    inbound: Option<Payload>,
    writer: Option<Cap<PortEvent>>,
    outbound: Option<Payload>,
}
impl Link {
    pub fn create(wire: &Cap<WireEvent>, nonce: u32) -> Cap<LinkEvent> {
        actor::create(Link {
            wire: wire.clone(),
            nonce,
            state: LinkState::Stop,
            balance: 0,
            reader: None,
            inbound: None,
            writer: None,
            outbound: None,
        })
    }
}
impl Actor for Link {
    type Event = LinkEvent;

    fn on_event(&mut self, event: Self::Event) {
        match &event {
            LinkEvent::Frame(frame) => {
                let tree_id = TreeId::new(self.nonce);
                if self.state == LinkState::Stop {
                    return; // EARLY EXIT WHEN LINK IS STOPPED.
                } else if frame.is_reset() {
                    self.state = LinkState::Init;
                    let nonce = frame.get_nonce();
                    println!("Link::nonce={}, frame.nonce={}", self.nonce, nonce);
                    if self.nonce < nonce {
                        println!("waiting...");
                    } else if self.nonce > nonce {
                        println!("entangle...");
                        let reply = Frame::new_entangled(&tree_id, frame::TICK, frame::TICK);
                        self.wire.send(WireEvent::new_frame(&reply));
                    } else {
                        println!("collision...");
                        self.nonce = rand::thread_rng().gen();
                        let reply = Frame::new_reset(self.nonce);
                        self.wire.send(WireEvent::new_frame(&reply));
                    }
                } else if frame.is_entangled() {
                    self.state = LinkState::Live;
                    let i_state = frame.get_i_state();
                    //println!("entangled i={}", i_state);
                    match i_state {
                        frame::TICK => {
                            //println!("TICK rcvd."); // liveness recv'd
                            if self.balance == 1 {
                                // receive completed
                                println!("TICK w/ surplus");
                                if let Some(reader) = &self.reader {
                                    if let Some(payload) = &self.inbound {
                                        reader.send( // release payload
                                            PortEvent::new_link_to_port_write(&payload)
                                        );
                                        self.reader = None; // reader satisfied
                                        self.inbound = None; // clear inbound
                                        self.balance = 0; // clear balance
                                    }
                                }
                            }
                            assert_eq!(self.balance, 0); // at this point, the balance should always be 0
                            match &self.outbound {
                                None => {
                                    let reply = Frame::new_entangled(
                                        &tree_id,
                                        frame::TICK, // liveness
                                        i_state,
                                    );
                                    self.wire.send(WireEvent::new_frame(&reply));
                                }
                                Some(payload) => {
                                    let mut reply = Frame::new_entangled(
                                        &tree_id,
                                        frame::TECK, // begin AIT
                                        i_state,
                                    );
                                    reply.set_payload(&payload);
                                    self.wire.send(WireEvent::new_frame(&reply));
                                    self.balance = -1; // deficit balance
                                }
                            }
                        }
                        frame::TECK => {
                            println!("TECK rcvd."); // begin AIT recv'd
                            match &self.reader {
                                Some(_cust) => {
                                    // reader ready
                                    self.inbound = Some(frame.get_payload());
                                    let reply = Frame::new_entangled(
                                        &tree_id,
                                        frame::TACK, // Ack AIT
                                        i_state,
                                    );
                                    self.wire.send(WireEvent::new_frame(&reply));
                                    self.balance = 1; // surplus balance
                                }
                                None => {
                                    // no reader ready
                                    let reply = Frame::new_entangled(
                                        &tree_id,
                                        frame::RTECK, // reject AIT
                                        i_state,
                                    );
                                    self.wire.send(WireEvent::new_frame(&reply));
                                    //self.balance = 0; // balance already clear?
                                    assert_eq!(self.balance, 0);
                                }
                            }
                        }
                        frame::TACK => {
                            println!("TACK rcvd."); // Ack AIT recv'd
                            assert_eq!(self.balance, -1); // deficit expected
                            println!("TACK w/ deficit");
                            if let Some(writer) = &self.writer {
                                writer.send(PortEvent::new_link_to_port_read()); // acknowlege write
                                self.writer = None; // writer satisfied
                                self.outbound = None; // clear outbound
                                self.balance = 0; // clear balance
                                let reply = Frame::new_entangled(
                                    &tree_id,
                                    frame::TICK, // liveness (Ack Ack)
                                    i_state,
                                );
                                self.wire.send(WireEvent::new_frame(&reply));
                            }
                        }
                        frame::RTECK => {
                            println!("RTECK rcvd."); // Reject AIT recv'd
                            let reply = Frame::new_entangled(
                                &tree_id,
                                frame::TICK, // liveness
                                i_state,
                            );
                            self.wire.send(WireEvent::new_frame(&reply));
                            self.balance = 0; // clear deficit
                        }
                        _ => {
                            panic!("bad protocol state");
                        }
                    }
                } else {
                    panic!("bad frame format");
                }
            }
            LinkEvent::Start(cust) => {
                let init = Frame::new_reset(self.nonce);
                self.wire.send(WireEvent::new_frame(&init)); // send init/reset
                self.state = LinkState::Init;
                cust.send(PortEvent::new_link_status(&self.state, &self.balance));
            }
            LinkEvent::Poll(cust) => {
                cust.send(PortEvent::new_link_status(&self.state, &self.balance));
                if self.state == LinkState::Live {
                    self.state = LinkState::Run; // clear Live status
                }
            }
            LinkEvent::Stop(cust) => {
                self.state = LinkState::Stop;
                cust.send(PortEvent::new_link_status(&self.state, &self.balance));
            }
            LinkEvent::Read(cust) => match &self.reader {
                None => {
                    self.reader = Some(cust.clone());
                }
                Some(_cust) => panic!("Only one Link-to-Port reader allowed"),
            },
            LinkEvent::Write(cust, payload) => match &self.writer {
                None => {
                    self.outbound = Some(payload.clone());
                    self.writer = Some(cust.clone());
                }
                Some(_cust) => panic!("Only one Port-to-Link writer allowed"),
            },
        }
    }
}

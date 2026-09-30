//! `xlights_networks.xml`: controllers, their outputs and the absolute channel layout.
//!
//! xLights numbers channels by walking every controller in file order and every output
//! (`<network>`) inside it, each consuming `MaxChannels` channels. A controller's start
//! channel is therefore the sum of everything before it, plus one.

use roxmltree::{Document, Node};

/// One output (`<network>` element): a universe for E1.31/Art-Net, the whole controller
/// for DDP, a serial port, ...
#[derive(Debug, Clone, PartialEq)]
pub struct NetOutput {
    /// `NetworkType` (`E131`, `DDP`, `ArtNet`, `NULL`, `DMX`, ...).
    pub kind: String,
    /// Universe (E1.31/Art-Net) or id, from `BaudRate`; 0 if not numeric.
    pub universe: u32,
    /// IP address (from `ComPort` or the controller's `IP`).
    pub ip: Option<String>,
    /// 1-based absolute start channel.
    pub start: u32,
    /// Channel count.
    pub channels: u32,
}

/// A controller from `xlights_networks.xml`.
#[derive(Debug, Clone, PartialEq)]
pub struct NetController {
    /// Controller name (what models reference with `Controller=` / `!Name:ch`).
    pub name: String,
    /// `Type` attribute: `Ethernet`, `Serial`, `Null`, ...
    pub kind: String,
    /// IP address for Ethernet controllers.
    pub ip: Option<String>,
    /// Protocol (`DDP`, `E131`, `ArtNet`, ...).
    pub protocol: Option<String>,
    /// Vendor / model, informational.
    pub vendor: Option<String>,
    /// Vendor model name.
    pub model: Option<String>,
    /// Active state.
    pub active: bool,
    /// 1-based absolute start channel.
    pub start: u32,
    /// Total channels.
    pub channels: u32,
    /// Outputs in order.
    pub outputs: Vec<NetOutput>,
}

impl NetController {
    /// Last channel (1-based, inclusive); `start - 1` for an empty controller.
    pub fn end(&self) -> u32 {
        (self.start + self.channels).saturating_sub(1)
    }
}

/// All controllers in channel order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Networks {
    /// Controllers in file (= channel) order.
    pub controllers: Vec<NetController>,
}

fn attr<'a>(n: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    n.attribute(name).map(str::trim).filter(|s| !s.is_empty())
}

fn attr_u32(n: Node, name: &str) -> Option<u32> {
    attr(n, name).and_then(|s| s.parse::<i64>().ok()).map(|v| v.clamp(0, u32::MAX as i64) as u32)
}

impl Networks {
    /// Parse `xlights_networks.xml`. Unknown elements are ignored.
    pub fn parse(xml: &str, warnings: &mut Vec<String>) -> Result<Networks, roxmltree::Error> {
        let doc = Document::parse_with_options(
            xml,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )?;
        let root = doc.root_element();
        let mut controllers = Vec::new();
        let mut next = 1u32;
        let mut legacy_index = 0;
        for child in root.children().filter(|c| c.is_element()) {
            match child.tag_name().name() {
                "Controller" => {
                    let name = attr(child, "Name")
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("Controller {}", controllers.len() + 1));
                    let ip = attr(child, "IP").map(str::to_string);
                    let active = match attr(child, "ActiveState") {
                        Some(s) => !s.eq_ignore_ascii_case("inactive"),
                        None => attr(child, "Active") != Some("0"),
                    };
                    let start = next;
                    let mut outputs = Vec::new();
                    for net in child
                        .children()
                        .filter(|c| c.is_element() && c.tag_name().name() == "network")
                    {
                        outputs.extend(parse_outputs(net, ip.as_deref(), &mut next));
                    }
                    if outputs.is_empty() {
                        warnings.push(format!(
                            "controller '{name}' in xlights_networks.xml has no outputs"
                        ));
                    }
                    let protocol = attr(child, "Protocol")
                        .map(str::to_string)
                        .or_else(|| outputs.first().map(|o| o.kind.clone()));
                    controllers.push(NetController {
                        name,
                        kind: attr(child, "Type").unwrap_or("Ethernet").to_string(),
                        ip,
                        protocol,
                        vendor: attr(child, "Vendor").map(str::to_string),
                        model: attr(child, "Model").map(str::to_string),
                        active,
                        start,
                        channels: next - start,
                        outputs,
                    });
                }
                "network" => {
                    // Pre-2020 files list outputs directly; treat each as a controller.
                    legacy_index += 1;
                    let start = next;
                    let outputs = parse_outputs(child, None, &mut next);
                    let first = outputs.first().cloned();
                    let name = attr(child, "Description")
                        .map(str::to_string)
                        .unwrap_or_else(|| match &first {
                            Some(o) => format!(
                                "{} {}{}",
                                o.kind,
                                o.ip.as_deref().map(|i| format!("{i} ")).unwrap_or_default(),
                                o.universe
                            ),
                            None => format!("Output {legacy_index}"),
                        });
                    controllers.push(NetController {
                        name,
                        kind: "Legacy".into(),
                        ip: first.as_ref().and_then(|o| o.ip.clone()),
                        protocol: first.as_ref().map(|o| o.kind.clone()),
                        vendor: None,
                        model: None,
                        active: attr(child, "Enabled") != Some("No"),
                        start,
                        channels: next - start,
                        outputs,
                    });
                }
                _ => {}
            }
        }
        Ok(Networks { controllers })
    }

    /// Controller by name (exact, then case-insensitive).
    pub fn controller(&self, name: &str) -> Option<&NetController> {
        let name = name.trim();
        self.controllers
            .iter()
            .find(|c| c.name == name)
            .or_else(|| self.controllers.iter().find(|c| c.name.eq_ignore_ascii_case(name)))
    }

    /// Controller whose channel range contains absolute channel `ch` (1-based).
    pub fn controller_for_channel(&self, ch: u32) -> Option<&NetController> {
        self.controllers
            .iter()
            .find(|c| c.channels > 0 && ch >= c.start && ch <= c.end())
    }

    /// Absolute channel for `#universe:ch` / `#ip:universe:ch` (1-based `ch`).
    pub fn universe_channel(&self, ip: Option<&str>, universe: u32, ch: u32) -> Option<u32> {
        self.controllers
            .iter()
            .flat_map(|c| c.outputs.iter())
            .find(|o| {
                o.universe == universe
                    && ip.map_or(true, |ip| {
                        o.ip.as_deref().is_some_and(|oip| oip.eq_ignore_ascii_case(ip))
                    })
            })
            .map(|o| o.start + ch.saturating_sub(1))
    }

    /// Total channels of all controllers.
    pub fn total_channels(&self) -> u32 {
        self.controllers.iter().map(|c| c.channels).sum()
    }
}

fn parse_outputs(net: Node, controller_ip: Option<&str>, next: &mut u32) -> Vec<NetOutput> {
    let kind = attr(net, "NetworkType").unwrap_or("NULL").to_string();
    let channels = attr_u32(net, "MaxChannels").unwrap_or(0);
    let universe = attr_u32(net, "BaudRate").unwrap_or(0);
    let ip = attr(net, "ComPort")
        .filter(|s| s.contains('.') || s.contains(':'))
        .map(str::to_string)
        .or_else(|| controller_ip.map(str::to_string));
    // Old E1.31 entries describe several consecutive universes in one element.
    let count = attr_u32(net, "NumUniverses").unwrap_or(1).clamp(1, 64_000);
    (0..count)
        .map(|i| {
            let o = NetOutput {
                kind: kind.clone(),
                universe: universe.saturating_add(i),
                ip: ip.clone(),
                start: *next,
                channels,
            };
            *next = next.saturating_add(channels);
            o
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Networks computer="SHOWPC">
  <Controller Id="1" Name="Leader" Description="" Type="Ethernet" Vendor="PixelPlus" Model="difftx"
              AutoSize="1" ActiveState="Active" AutoLayout="1" IP="192.168.1.50" Protocol="DDP">
    <network ChannelsPerPacket="1440" KeepChannelNumbers="1" NetworkType="DDP" MaxChannels="1500"
             Enabled="Yes" ComPort="192.168.1.50" BaudRate="1"/>
  </Controller>
  <Controller Id="2" Name="F16" Type="Ethernet" IP="10.0.0.9" Protocol="E131" ActiveState="Active">
    <network NetworkType="E131" MaxChannels="510" ComPort="10.0.0.9" BaudRate="100"/>
    <network NetworkType="E131" MaxChannels="510" ComPort="10.0.0.9" BaudRate="101"/>
  </Controller>
  <Controller Id="3" Name="Off" Type="Null" ActiveState="Inactive">
    <network NetworkType="NULL" MaxChannels="30"/>
  </Controller>
</Networks>"#;

    #[test]
    fn channel_layout() {
        let mut w = vec![];
        let n = Networks::parse(XML, &mut w).unwrap();
        assert!(w.is_empty());
        assert_eq!(n.controllers.len(), 3);
        let l = n.controller("leader").unwrap();
        assert_eq!((l.start, l.channels, l.end()), (1, 1500, 1500));
        assert_eq!(l.protocol.as_deref(), Some("DDP"));
        let f = n.controller("F16").unwrap();
        assert_eq!((f.start, f.channels), (1501, 1020));
        assert!(!n.controller("Off").unwrap().active);
        assert_eq!(n.controller_for_channel(1501).unwrap().name, "F16");
        assert_eq!(n.universe_channel(None, 101, 1), Some(2011));
        assert_eq!(n.universe_channel(Some("10.0.0.9"), 100, 3), Some(1503));
        assert_eq!(n.universe_channel(Some("1.2.3.4"), 100, 3), None);
        assert_eq!(n.total_channels(), 2550);
    }

    #[test]
    fn legacy_and_garbage() {
        let mut w = vec![];
        let n = Networks::parse(
            r#"<Networks><network NetworkType="E131" ComPort="MULTICAST" BaudRate="1" MaxChannels="512" NumUniverses="3"/></Networks>"#,
            &mut w,
        )
        .unwrap();
        assert_eq!(n.controllers[0].channels, 1536);
        assert_eq!(n.universe_channel(None, 3, 1), Some(1025));
        assert!(Networks::parse("<Networks><oops", &mut w).is_err());
    }
}

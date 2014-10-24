#![crate_type="bin"]
#![feature(macro_rules)]

extern crate rustirc;
use rustirc::{Connection, IrcEventHandler, IrcWriter, IrcEvent};
use std::io::{TcpStream, IoResult, timer, IoError, BufferedStream};
use std::sync::{Mutex, Arc};
use poll::Poll;
use std::from_str::from_str;

mod poll;


macro_rules! ioassume(
    ($e:expr, $msg:expr) => (match $e { Ok(e) => e, Err(msg) => fail!($msg, msg) })
)



pub struct ProtocollingStream {
	inner: BufferedStream<TcpStream>
}

impl ProtocollingStream {
	pub fn new(inner: TcpStream) -> ProtocollingStream {
		ProtocollingStream {inner: BufferedStream::new(inner)}
	}
}

impl std::io::Writer for ProtocollingStream {
	fn write(&mut self, buf: &[u8]) -> IoResult<()> {
		try!(self.inner.write(buf));
		self.inner.flush()
	}
}

impl rustirc::CloseWrite for ProtocollingStream {
	fn close_write(&mut self) -> IoResult<()> {
		self.inner.close_write()
	}
}

impl Clone for ProtocollingStream {
	fn clone(&self) -> ProtocollingStream { ProtocollingStream {inner: self.inner.clone()} }
}

pub struct NickGenerator {
	basename: &'static str,
	attempt: uint
}

impl Iterator<String> for NickGenerator {
	fn next(&mut self) -> Option<String> {
		self.attempt += 1;
		Some(if self.attempt > 1 {
			format!("{}{}", self.basename, self.attempt)
		} else {
			self.basename.to_string()
		})
	}
}

pub struct MessageContext<'a, W: IrcWriter + 'a> {
	write: &'a mut W,
	sender: &'a str,
	channel: Option<&'a str>
}

impl<'a, W: IrcWriter> MessageContext<'a, W> {
	pub fn new(write: &'a mut W, sender: &'a str, channel: Option<&'a str>) -> MessageContext<'a, W> {
		MessageContext {write: write, sender: sender, channel: channel}
	}
	pub fn reply(&mut self, text: &str) -> IoResult<()> {
		match self.channel {
			Some(channel) => self.write.channel_notice(channel, text),
			None => self.write.notice(self.sender, text)
		}
	}
	pub fn channel_reply(&mut self, text: &str) -> IoResult<()> {
		match self.channel {
			Some(channel) => self.write.channel_notice(channel, text),
			None => Err(IoError {kind: std::io::OtherIoError,
					desc: "cannot reply to channel of direct message",
					detail: None})
		}
	}
	pub fn private_reply(&mut self, text: &str) -> IoResult<()> {
		self.write.notice(self.sender, text)
	}
	pub fn clone_inner(&self) -> W {
		self.write.clone()
	}
	pub fn unwrap(self) -> &'a mut W {
		self.write
	}
	pub fn is_private(&self) -> bool {
		self.channel == None
	}
}



struct Bot {
	cmd_marker: &'static str,
	poll: Arc<Mutex<Option<Poll>>>,
	channel: String
}

impl Bot {
	fn is_command<'a>(&self, text: &'a str) -> Option<(&'a str, &'a str)> {
		if text.slice_to(1) == self.cmd_marker {
			let mut iter = text.slice_from(1).splitn(1, |c: char| c == ' ');
			let cmd = match iter.next() {
				Some(x) => x,
				None => fail!("str.split yielded less than one slice")
			};
			let args = match iter.next() {
				Some(x) => x.trim_chars(|c: char| c.is_whitespace()),
				None => ""
			};
			Some((cmd, args))
		} else {
			None
		}
	}
	fn start_poll<'a, W: IrcWriter>(&mut self, mut ctx: MessageContext<'a, W>, args: &str) -> IoResult<()> {
		let new_poll: Poll = match from_str(args) {
				Some(x) => x,
				None => {
					return ctx.reply("usage: poll <minutes>|<question>|<answer 1>|<answer 2> ...");
				}
			};
		let dur = new_poll.duration();
		let write = ctx.unwrap();
		try!(write.channel_notice(self.channel.as_slice(), format!("Poll started: {}", new_poll.name()).as_slice()));
		let mut i = 1u;
		for (_, name) in new_poll.answers() {
			try!(write.channel_notice(self.channel.as_slice(), format!("  {}: {}", i, name).as_slice()));
			i += 1;
		}
		let mut poll = self.poll.lock();
		*poll = Some(new_poll);
		
		let mutex = self.poll.clone();
		let channel = self.channel.clone();
		let mut outclone = write.clone();
		std::task::spawn(proc() {
			timer::sleep(dur);
			let mut poll = mutex.lock();
			let result = poll.clone();
			*poll = None;
			match result {
				Some(x) => {
					match x.evaluate(channel.as_slice(), &mut outclone) {
						Ok(()) => {},
						Err(x) => fail!(x)
					}
				},
				None => {
					println!("poll ended early");
				}
			}
		});
		Ok(())
	}
	fn vote<'a, W: IrcWriter>(&mut self, mut ctx: MessageContext<'a, W>, args: &str) -> IoResult<()> {
		let num: uint = match from_str::<uint>(args.trim_chars(|c: char| c.is_whitespace())) {
			Some(x) => x - 1u,
			None => {
				return ctx.reply("usage: vote <number>");
			}
		};
		let mut mutex = self.poll.lock();
		let poll = match (*mutex).as_mut() {
			Some(x) => x,
			None => {
				return ctx.reply("no poll running right now");
			}
		};
		if num >= poll.num_answers() {
			return ctx.reply(format!("choose a number from 1 and {}", poll.num_answers()).as_slice());
		}
		poll.add_vote(num);
		Ok(())
	}
	fn end_poll<'a, W: IrcWriter>(&mut self, mut ctx: MessageContext<'a, W>) -> IoResult<()> {
		{
			let mut poll = self.poll.lock();
			let result = poll.clone();
			*poll = None;
			match result {
				Some(x) => {
					try!(x.evaluate(self.channel.as_slice(), ctx.unwrap()));
				},
				None => {
					try!(ctx.reply("no poll running"));
				}
			}
		}
		self.poll = Arc::new(Mutex::new(None));
		Ok(())
	}
	fn handle_command<'a, W: IrcWriter>(&mut self, mut ctx: MessageContext<'a, W>, cmd: &str, args: &str) -> IoResult<()> {
		match cmd {
			"kill" => ctx.unwrap().quit(),
			"poll" => self.start_poll(ctx, args),
			"vote" => self.vote(ctx, args),
			"endpoll" => self.end_poll(ctx),
			_ => ctx.private_reply(format!("unknown command: {}", cmd).as_slice())
		}
	}
}

impl IrcEventHandler for Bot {
	fn on_registered<W: IrcWriter>(&mut self, write: &mut W) -> IoResult<()> {
		try!(write.join(self.channel.as_slice()));
		Ok(())
	}
	fn on_privmsg<'a, W: IrcWriter>(&mut self, text: &str, ev: &IrcEvent<'a>, write: &mut W) -> IoResult<()> {
		let ctx = MessageContext::new(write, ev.sender, None);
		match self.is_command(text) {
			Some((cmd, args)) => self.handle_command(ctx, cmd, args),
			None => Ok(())
		}
	}
}

pub fn main() {
	let tcp = ioassume!(TcpStream::connect("irc.quakenet.org", 6667), "TCP Connection failed: {}");
	let protocol = ProtocollingStream::new(tcp);
	let mut conn = ioassume!(Connection::connect(
			protocol,
			NickGenerator {basename: "CrystalGBot", attempt:0},
			"CrystalGBot".to_string(),
			"CrystalGamma experimental chat bot implemented in Rust".to_string(),
			Bot {cmd_marker: "!", poll: Arc::new(Mutex::new(None)), channel: "#crystalgamma".to_string()}),
		"IRC connection failed: {}");
	ioassume!(conn.eventloop(),"main loop failed: {}");
}
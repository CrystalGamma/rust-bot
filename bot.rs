#![crate_type="bin"]
#![feature(macro_rules)]

extern crate rustirc;
use rustirc::{Connection, IrcEventHandler, IrcWriter, IrcEvent};
use std::io::{TcpStream, IoResult, timer, IoError, BufferedReader};
use std::sync::{Mutex, Arc};
use poll::Poll;
use std::from_str::from_str;

mod poll;


macro_rules! ioassume(
    ($e:expr, $msg:expr) => (match $e { Ok(e) => e, Err(msg) => fail!($msg, msg) })
)

pub struct NickGenerator {
	basename: String,
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

pub struct MessageContext<'a, W: IrcWriter> {
	write: &'a mut W,
	sender: &'a str,
	channel: Option<&'a str>
}

impl<'a, W: IrcWriter> MessageContext<'a, W> {
	pub fn new(write: &'a mut W, sender: &'a str, channel: Option<&'a str>) -> MessageContext<'a, W> {
		MessageContext {write: write, sender: sender, channel: channel}
	}
	pub fn reply(&self, text: &str) -> IoResult<()> {
		match self.channel {
			Some(channel) => self.write.channel_notice(channel, text),
			None => self.write.notice(self.sender, text)
		}
	}
	pub fn channel_reply(&self, text: &str) -> IoResult<()> {
		match self.channel {
			Some(channel) => self.write.channel_notice(channel, text),
			None => Err(IoError {kind: std::io::OtherIoError,
					desc: "cannot reply to channel of direct message",
					detail: None})
		}
	}
	pub fn private_reply(&self, text: &str) -> IoResult<()> {
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
		if text.starts_with(self.cmd_marker) {
			let mut iter = text.slice_from(self.cmd_marker.len()).splitn(1, |c: char| c == ' ');
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
	fn start_poll<'a, W: IrcWriter>(&mut self, ctx: MessageContext<'a, W>, args: &str) -> IoResult<()> {
		let new_poll: Poll = match from_str(args) {
				Some(x) => x,
				None => {
					return ctx.reply("usage: poll <minutes>|<question>|<answer 1>|<answer 2> ...");
				}
			};
		let dur = new_poll.duration();
		let write: &mut W = ctx.unwrap();
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
	fn vote<'a, W: IrcWriter>(&mut self, ctx: MessageContext<'a, W>, args: &str) -> IoResult<()> {
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
		ctx.private_reply("vote counted")
	}
	fn end_poll<'a, W: IrcWriter>(&mut self, ctx: MessageContext<'a, W>) -> IoResult<()> {
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
	fn handle_command<'a, W: IrcWriter>(&mut self, ctx: MessageContext<'a, W>, cmd: &str, args: &str) -> IoResult<()> {
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
	let args = std::os::args();
	let mut iter = args.iter();
	let mut name = "CrystalGBot".to_string();
	let mut server = "irc.quakenet.org".to_string();
	let mut port = 6667u16;
	let mut channel = "#crystalgamma".to_string();
	iter.next();
	loop {
		match iter.next() {
			Some(arg) => match arg.as_slice() {
				"-s" | "--server" => match iter.next() {
					Some(serv) => { server = serv.clone(); },
					None => fail!("incomplete command line: expecting server name after {}", arg)
				},
				"-p" | "--port" => match iter.next() {
					Some(s) => match from_str(s.as_slice()) {
						Some(num) => { port = num; }
						None => fail!("{} is not a valid port number", s)
					},
					None => fail!("incomplete command line: expecting number after {}", arg)
				},
				"-n" | "--nick" | "--name" => match iter.next() {
					Some(nick) => { name = nick.clone(); },
					None =>	fail!("incomplete command line: expecting nick name after {}", arg)
				},
				"-c" | "--channel" => match iter.next() {
					Some(chan) => { channel = chan.clone(); },
					None =>	fail!("incomplete command line: expecting channel name after {}", arg)
				},
				_ => fail!("unknown command line option: {}", arg)
			},
			None => break
		}
	}
	let tcp = ioassume!(TcpStream::connect(server.as_slice(), port), "TCP Connection failed: {}");
	let mut conn = ioassume!(Connection::connect(
			BufferedReader::new(tcp.clone()),tcp,
			NickGenerator {basename: name.clone(), attempt:0},
			name,
			"CrystalGamma experimental chat bot implemented in Rust".to_string(),
			Bot {
				cmd_marker: "~",
				poll: Arc::new(Mutex::new(None)),
				channel: channel
			}), "IRC connection failed: {}");
	ioassume!(conn.eventloop(),"main loop failed: {}");
}
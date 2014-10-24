use std::io::IoResult;
use std::time::Duration;
use rustirc::IrcWriter;
use std;


macro_rules! tof32( ($e:expr) => (match $e.to_f32() {Some(x)=>x,None=>fail!("conversion to float failed")}))

pub struct Answer {
	pub supporters: uint,
	pub name: String
}

impl Clone for Answer {
	fn clone(&self) -> Answer {
		Answer {supporters: self.supporters, name: self.name.clone()}
	}
}

pub struct Poll {
	name: String,
	answers: Vec<Answer>,
	duration: Duration
}

impl Clone for Poll {
	fn clone(&self) -> Poll {
		Poll {name: self.name.clone(), answers: self.answers.clone(), duration: self.duration}
	}
}


impl Poll {
	pub fn evaluate<W: IrcWriter>(&self, channel: &str, write: &mut W) -> IoResult<()> {
		let totalvotes = self.answers.iter().fold(0u, |sum: uint, entry: &Answer| sum + entry.supporters);
		if totalvotes > 0 {
			try!(write.channel_notice(channel, format!("Poll result for '{}'", self.name).as_slice()));
			let mut i = 0u;
			for entry in self.answers.iter() {
				try!(write.channel_notice(channel, format!(
					"#{}: {}: {} votes ({:.2f}%)",
					i,
					entry.name,
					entry.supporters,
					tof32!(entry.supporters)/tof32!(totalvotes)*100.0f32
				).as_slice()));
				i += 1;
			}
			Ok(())
		} else {
			write.channel_notice(channel, format!("No response to '{}'", self.name).as_slice())
		}
	}
	
	pub fn name<'a>(&'a self) -> &str { self.name.as_slice() }
	
	pub fn answers<'a>(&'a self) -> std::iter::Map<&Answer, (uint, &str), std::slice::Items<Answer>> {
		self.answers.iter().map(|x: &'a Answer| (x.supporters, x.name.as_slice()))
	}
	
	pub fn add_vote(&mut self, num: uint) {
		assert!(num < self.answers.len());
		self.answers.get_mut(num).supporters += 1;
	}
	
	pub fn duration(&self) -> Duration { self.duration }
	pub fn num_answers(&self) -> uint { self.answers.len() }
}

impl std::from_str::FromStr for Poll {
	fn from_str(args: &str) -> Option<Poll> {
		let mut segments = args.split(|c: char| c == '|')
				.map(|s: &str| s.trim_chars(|c: char| c.is_whitespace()).to_string());
		let time: i64 = match segments.next() {
			Some(segment) => {
				match from_str::<i64>(segment.as_slice()) {
				Some(num) if num > 0 => num,
				_ => {return None;}
				}
			},
			None => fail!("str.split did not yield any slices")
		};
		let title = match segments.next() {
			Some(x) => {
				if x.len() != 0 {
					x
				} else {
					return None;
				}
			},
			None => {return None;}
		};
		let answers: Vec<Answer> = segments.map(|s: String| Answer{supporters: 0u, name: s}).collect();
		if answers.len() < 2 {
			return None;
		}
		Some(Poll {name: title, answers: answers, duration: Duration::minutes(time)})
	}
}
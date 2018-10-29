use rocket;
use rocket::http::hyper::header::*;
use rocket::response::{self, Responder};
use std::cmp;
use std::convert::From;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::str::FromStr;

pub enum PartialFileRange {
	AllFrom(u64),
	FromTo(u64, u64),
	Last(u64),
}

impl From<ByteRangeSpec> for PartialFileRange {
	fn from(b: ByteRangeSpec) -> PartialFileRange {
		match b {
			ByteRangeSpec::AllFrom(from) => PartialFileRange::AllFrom(from),
			ByteRangeSpec::FromTo(from, to) => PartialFileRange::FromTo(from, to),
			ByteRangeSpec::Last(last) => PartialFileRange::Last(last),
		}
	}
}

impl From<Vec<ByteRangeSpec>> for PartialFileRange {
	fn from(v: Vec<ByteRangeSpec>) -> PartialFileRange {
		match v.into_iter().next() {
			None => PartialFileRange::AllFrom(0),
			Some(byte_range) => PartialFileRange::from(byte_range),
		}
	}
}

pub struct RangeResponder<R> {
	original: R,
}

impl<'r, R: Responder<'r>> RangeResponder<R> {
	pub fn new(original: R) -> RangeResponder<R> {
		RangeResponder { original }
	}

	fn ignore_range(self, request: &rocket::request::Request) -> response::Result<'r> {
		let mut response = self.original.respond_to(request)?;
		response.set_status(rocket::http::Status::RangeNotSatisfiable);
		Ok(response)
	}
}

fn truncate_range(range: &PartialFileRange, file_length: &Option<u64>) -> Option<(u64, u64)> {
	use self::PartialFileRange::*;

	match (range, file_length) {
		(FromTo(from, to), Some(file_length)) => {
			if from <= to && from < file_length {
				Some((*from, cmp::min(*to, file_length - 1)))
			} else {
				None
			}
		}
		(AllFrom(from), Some(file_length)) => {
			if from < file_length {
				Some((*from, file_length - 1))
			} else {
				None
			}
		}
		(Last(last), Some(file_length)) => {
			if last < file_length {
				Some((file_length - last, file_length - 1))
			} else {
				Some((0, file_length - 1))
			}
		}
		(_, None) => None,
	}
}

impl<'r> Responder<'r> for RangeResponder<File> {
	fn respond_to(mut self, request: &rocket::request::Request) -> response::Result<'r> {
		let range_header = request.headers().get_one("Range");
		let range_header = match range_header {
			None => return Ok(self.original.respond_to(request)?),
			Some(h) => h,
		};

		let vec_range = match Range::from_str(range_header) {
			Ok(Range::Bytes(v)) => v,
			_ => return self.ignore_range(request),
		};

		let partial_file_range = match vec_range.into_iter().next() {
			None => PartialFileRange::AllFrom(0),
			Some(byte_range) => PartialFileRange::from(byte_range),
		};

		let metadata: Option<_> = self.original.metadata().ok();
		let file_length: Option<u64> = metadata.map(|m| m.len());
		let range: Option<(u64, u64)> = truncate_range(&partial_file_range, &file_length);

		if let Some((from, to)) = range {
			let content_range = ContentRange(ContentRangeSpec::Bytes {
				range: range,
				instance_length: file_length,
			});
			let content_len = to - from + 1;

			match self.original.seek(SeekFrom::Start(from)) {
				Ok(_) => (),
				Err(_) => return Err(rocket::http::Status::InternalServerError),
			}
			let partial_original = self.original.take(content_len).into_inner();
			let mut response = partial_original.respond_to(request)?;
			response.set_header(ContentLength(content_len));
			response.set_header(content_range);
			response.set_status(rocket::http::Status::PartialContent);

			Ok(response)
		} else {
			self.ignore_range(request)
		}
	}
}

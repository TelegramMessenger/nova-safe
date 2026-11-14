function check(name, actual, expected) {
  if (!Object.is(actual, expected)) {
    throw new Error(name + ": expected " + String(expected) + ", got " + String(actual));
  }
}

var t = Date.UTC(2026, 10, 14, 5, 36, 26, 123);
var seconds = t - 123;
var d = new Date(t);
check("toString round trip", Date.parse(d.toString()), seconds);
check("toUTCString round trip", Date.parse(d.toUTCString()), seconds);
check("toISOString round trip", Date.parse(d.toISOString()), t);
check("toDateString round trip", Date.parse(d.toDateString()), new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime());
check("Date() string", Math.abs(Date.parse(Date()) - Date.now()) < 2000, true);

check("toString with a zone name", Date.parse("Sat Nov 14 2026 09:36:26 GMT+0400 (Gulf Standard Time)"), seconds);
check("toString without a zone name", Date.parse("Sat Nov 14 2026 09:36:26 GMT+0400"), seconds);
check("negative offset", Date.parse("Sat Nov 14 2026 09:36:26 GMT-0530 (India Standard Time)"), Date.UTC(2026, 10, 14, 15, 6, 26));
check("offset with a colon", Date.parse("Sat Nov 14 2026 09:36:26 GMT+04:00"), seconds);
check("nested parentheses", Date.parse("Sat Nov 14 2026 09:36:26 GMT+0400 (a (b) c)"), seconds);
check("unbalanced parenthesis", Date.parse("Sat Nov 14 2026 09:36:26 GMT+0400 (Gulf"), NaN);
check("closing parenthesis", Date.parse("Sat Nov 14 2026 09:36:26 GMT+0400 )"), NaN);
check("text after the zone name", Date.parse("Sat Nov 14 2026 09:36:26 GMT+0400 (Gulf) extra"), NaN);

check("RFC 2822 GMT", Date.parse("Sat, 14 Nov 2026 05:36:26 GMT"), seconds);
check("RFC 2822 +0000", Date.parse("Sat, 14 Nov 2026 05:36:26 +0000"), seconds);
check("RFC 2822 -0800", Date.parse("Sat, 14 Nov 2026 05:36:26 -0800"), Date.UTC(2026, 10, 14, 13, 36, 26));
check("RFC 2822 two-digit year", Date.parse("Sat, 14 Nov 26 05:36:26 GMT"), seconds);
check("epoch", Date.parse("Thu, 01 Jan 1970 00:00:00 GMT"), 0);
check("year 1", Date.parse("Mon Jan 01 0001 00:00:00 GMT+0000"), -62135596800000);
check("negative year", Date.parse("Fri Dec 31 -000001 23:59:59 GMT+0000"), -62167219201000);

check("day month year", Date.parse("14 Nov 2026 05:36:26 GMT"), seconds);
check("month day year", Date.parse("Nov 14 2026 05:36:26 GMT"), seconds);
check("month day, year UTC", Date.parse("Nov 14, 2026 05:36:26 UTC"), seconds);
check("full month name", Date.parse("November 14, 2026 05:36:26 UTC"), seconds);
check("12-hour PM", Date.parse("Sat Nov 14 2026 9:36 PM GMT"), Date.UTC(2026, 10, 14, 21, 36));
check("12 AM", Date.parse("Nov 14 2026 12:00 AM UTC"), Date.UTC(2026, 10, 14));
check("13 AM", Date.parse("Nov 14 2026 13:00 AM UTC"), NaN);
check("EST", Date.parse("Sat Nov 14 2026 09:36:26 EST"), Date.UTC(2026, 10, 14, 14, 36, 26));
check("PDT", Date.parse("Sat Nov 14 2026 09:36:26 PDT"), Date.UTC(2026, 10, 14, 16, 36, 26));
check("CEST", Date.parse("Sat Nov 14 2026 09:36:26 CEST"), Date.UTC(2026, 10, 14, 7, 36, 26));
check("slash date with an offset", Date.parse("11/14/2026 05:36:26 GMT+0200"), Date.UTC(2026, 10, 14, 3, 36, 26));
check("minus sign", Date.parse("Sat Nov 14 2026 09:36:26 GMT−0400"), Date.UTC(2026, 10, 14, 13, 36, 26));
check("no-break spaces", Date.parse("Sat Nov 14　 2026 09:36:26 GMT"), Date.UTC(2026, 10, 14, 9, 36, 26));
check("line separator", Date.parse("Sat Nov 14 2026 09:36:26 GMT"), NaN);

check("local month day year", Date.parse("Nov 14 2026"), new Date(2026, 10, 14).getTime());
check("local slash date", Date.parse("11/14/2026 05:36:26"), new Date(2026, 10, 14, 5, 36, 26).getTime());
check("local year first", Date.parse("2026/11/14 05:36"), new Date(2026, 10, 14, 5, 36).getTime());
check("local ISO-like with a space", Date.parse("2026-11-14 05:36:26"), new Date(2026, 10, 14, 5, 36, 26).getTime());
check("month and year", Date.parse("Nov 2026"), new Date(2026, 10, 1).getTime());

check("one fraction digit", Date.parse("2026-11-14T05:36:26.1Z"), Date.UTC(2026, 10, 14, 5, 36, 26, 100));
check("nine fraction digits", Date.parse("2026-11-14T05:36:26.123456789Z"), t);
check("hour 24", Date.parse("2026-11-14T24:00:00Z"), Date.UTC(2026, 10, 15));
check("hour 24 and a millisecond", Date.parse("2026-11-14T24:00:00.001Z"), NaN);
check("hour 24 and a minute", Date.parse("2026-11-14T24:01Z"), NaN);
check("T without a time", Date.parse("2026-11-14T5:36"), NaN);

check("garbage", Date.parse("garbage"), NaN);
check("empty", Date.parse(""), NaN);
check("32 is a year", Date.parse("Nov 32 00:00 GMT"), Date.UTC(2032, 10, 1));
check("hour 25", Date.parse("Nov 14 2026 25:00 GMT"), NaN);
check("only the first 127 characters", Date.parse("Nov 14 2026 " + " ".repeat(200) + "05:36 GMT"), new Date(2026, 10, 14).getTime());
check("long word", Date.parse("a".repeat(1000)), NaN);
check("many digits", Date.parse("Nov 14 1234567890"), NaN);

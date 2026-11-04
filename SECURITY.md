# Security

## Reporting a vulnerability

Please report it privately, not in a public issue: on GitHub, open the
repository's **Security** tab and choose **Report a vulnerability**. Say what
you found, how to reproduce it (a sample file helps most), and what it lets an
attacker do.

You get an answer within 7 days. The fix is made in private and released as
soon as it is ready; the report is then made public with credit to you, unless
you prefer otherwise.

## What counts

Aktis opens files made by others (RAW, DNG, JPEG, TIFF, DCP profiles, lens
files, presets, keyword lists, catalogs) and downloads AI models. A
vulnerability is, for example:

- a file that makes the program run code, or write outside the folder the
  user chose, or change a photo's original;
- a download that installs something other than the file the program expects;
- anything that sends the user's data off the computer.

A file that only makes the program stop, or show an error, is a bug: open an
ordinary issue.

What the program defends against and what it does not is written in
[docs/threat-model.md](docs/threat-model.md).

## Supported versions

Aktis is not released yet; fixes go to the `main` branch.

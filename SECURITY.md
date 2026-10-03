# Security policy

FERRUM reads files from untrusted sources (DICOM, NIfTI), talks to
segmentation engines over HTTP and serves tools to AI agents, so security
reports are welcome.

## Reporting a vulnerability

Please **do not open a public issue**. Report privately instead, either
through GitHub's *Report a vulnerability* button on the Security tab or by
e-mail to [research@vchukanov.ru](mailto:research@vchukanov.ru), with:

- the affected version or commit;
- steps or a file to reproduce (synthetic data only — never send patient
  data);
- the impact you expect.

You will get an acknowledgement within 7 days. Fixes are released as soon
as practical, and reporters are credited unless they ask otherwise.

## Supported versions

Only the latest release and the `develop` branch receive security fixes.

## Scope

In scope: crashes or memory exhaustion from crafted files, path traversal
or writes outside configured roots in `ferrum-cli`, identifiers leaking
into agent outputs or engine requests, and weaknesses in the engine
protocol client or the reference bridges.

FERRUM is research and engineering software, not a certified medical
device; see [docs/vision.md](docs/vision.md).

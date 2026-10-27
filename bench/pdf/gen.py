"""Writes the PDF benchmark's input: a 200-page report, every page 40 lines
of Helvetica text, content streams Flate-compressed, as a word processor
would write it. Standard library only, so the input is the same everywhere.

    python3 -I gen.py <dir>

Each line holds the word "invoice" once, so text extraction has a checksum:
200 pages x 40 lines = 8000 occurrences.
"""

import sys
import zlib

PAGES = 200
LINES = 40
WORDS = "alpha beta gamma delta epsilon zeta theta kappa lambda omicron sigma omega".split()


def content(page: int) -> bytes:
    out = ["BT /F1 10 Tf 12 TL 56 770 Td"]
    for line in range(LINES):
        words = " ".join(WORDS[(page + line + i) % len(WORDS)] for i in range(8))
        out.append(f"(Line {line + 1} of page {page + 1}: invoice {words}) Tj T*")
    out.append("ET")
    return zlib.compress("\n".join(out).encode("latin-1"), 6)


def main(directory: str) -> None:
    objects: list[bytes] = []
    kids = " ".join(f"{4 + 2 * i} 0 R" for i in range(PAGES))
    objects.append(b"<< /Type /Catalog /Pages 2 0 R >>")
    objects.append(
        f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} /MediaBox [0 0 612 792] "
        "/Resources << /Font << /F1 3 0 R >> >> >>".encode()
    )
    objects.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>")
    for page in range(PAGES):
        objects.append(f"<< /Type /Page /Parent 2 0 R /Contents {5 + 2 * page} 0 R >>".encode())
        body = content(page)
        objects.append(
            f"<< /Length {len(body)} /Filter /FlateDecode >>\nstream\n".encode() + body + b"\nendstream"
        )
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for number, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{number} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    for offset in offsets:
        out += f"{offset:010} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    with open(f"{directory}/report.pdf", "wb") as f:
        f.write(out)


if __name__ == "__main__":
    main(sys.argv[1])

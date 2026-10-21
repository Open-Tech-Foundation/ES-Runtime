# The esrun image: the released binary on a distroless base, nothing else.
#
# Built by .github/workflows/container.yml from the release's own archives
# (checksum-verified there), placed per architecture under bin/<arch>/ in the
# build context, so the image carries exactly the binary the release shipped.
#
# distroless cc-debian13: glibc 2.41 (esrun needs 2.39+) and the C runtime it
# links, no shell, no package manager, and a non-root user. esrun carries its
# own TLS roots, so no ca-certificates are needed.
FROM gcr.io/distroless/cc-debian13:nonroot
ARG TARGETARCH
COPY --chmod=0755 bin/${TARGETARCH}/esrun /usr/local/bin/esrun
WORKDIR /app
ENTRYPOINT ["/usr/local/bin/esrun"]

# The four games in a web browser, for any number of players at once.
#
#   docker compose up --build        # then open http://localhost:7681
#
# ttyd (https://github.com/tsl0922/ttyd) serves a terminal in the browser and
# starts a fresh `arcade` for every tab that connects. It's the only thing a
# session runs: all four games are built into it, it plays the chosen one's TUI,
# comes back to its menu when the game exits, and ends the session on Quit. With
# --session-saves, each session's saves are its own and gone when it ends. The
# image has nothing else in it (it's built FROM scratch: no shell, no other
# programs), so there's no command line to reach, from the browser or from
# `docker exec`.
#
# It builds for whatever platform Docker targets: linux/amd64 or linux/arm64,
# from a Linux, macOS or Windows host (`docker buildx build --platform
# linux/amd64,linux/arm64 .` makes both).

ARG RUST_VERSION=1.98
ARG ALPINE_VERSION=3.22
ARG TTYD_VERSION=1.7.7

FROM rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS build
ARG TTYD_VERSION
# Set by BuildKit to the platform being built for.
ARG TARGETARCH
# git and Python fetch and build the SCOWL word list (see build.rs); curl gets ttyd.
RUN apk add --no-cache musl-dev git python3 curl

# ttyd's static build for this platform, checked against its published sums.
RUN set -eu; \
    case "${TARGETARCH:-$(uname -m)}" in \
        amd64|x86_64) arch=x86_64 ;; \
        arm64|aarch64) arch=aarch64 ;; \
        *) echo "No ttyd build for ${TARGETARCH:-$(uname -m)}; use linux/amd64 or linux/arm64." >&2; exit 1 ;; \
    esac; \
    base="https://github.com/tsl0922/ttyd/releases/download/${TTYD_VERSION}"; \
    mkdir /ttyd && cd /ttyd; \
    curl -fsSL -o "ttyd.${arch}" "${base}/ttyd.${arch}"; \
    curl -fsSL "${base}/SHA256SUMS" | grep " ttyd.${arch}\$" | sha256sum -c -; \
    mv "ttyd.${arch}" ttyd && chmod 755 ttyd

# ttyd's own page, with the terminal filling the window. Its terminal (xterm.js)
# keeps a scrollbar's width free on the right even though the games never scroll
# (so `scrollback=0` below, which also stops it counting one when sizing), and
# its first fit can come before the page and its fonts are settled, which a
# resize then corrects; so the page fits again once they are. The leftover pixels
# short of a whole column get the terminal's black. The page is taken from this
# ttyd itself, so it always matches the version pinned above.
RUN set -eu; \
    /ttyd/ttyd --port 7999 true & pid=$!; \
    for _ in $(seq 50); do curl -fsS -o /ttyd/index.html http://127.0.0.1:7999/ && break; sleep 0.1; done; \
    kill "$pid"; \
    grep -q '</head>' /ttyd/index.html; \
    style='<style>html,body{background:\#000;margin:0}.xterm .xterm-viewport{overflow-y:hidden!important}</style>'; \
    refit='<script>(function(){function f(){window.dispatchEvent(new Event("resize"))}window.addEventListener("load",function(){[50,300,1000].forEach(function(t){setTimeout(f,t)})});if(document.fonts){document.fonts.ready.then(f)}})()</script>'; \
    sed -i "s#</head>#${style}${refit}</head>#" /ttyd/index.html; \
    grep -q 'overflow-y:hidden!important' /ttyd/index.html; \
    grep -q 'dispatchEvent(new Event("resize"))' /ttyd/index.html

WORKDIR /src
COPY . .
# Alpine's Rust targets musl, so the games are static and need nothing else.
# Only `arcade` ships: it has all four games in it.
RUN cargo build --release --locked --bin arcade \
    && mkdir -p /out/tmp \
    && cp target/release/arcade /out/arcade \
    && chmod 1777 /out/tmp

FROM scratch
# The source label links the image to its repository on GHCR.
LABEL org.opencontainers.image.title="Jotto arcade" \
      org.opencontainers.image.description="Jotto, Wordle, Contexto and Flights in a web browser" \
      org.opencontainers.image.source="https://github.com/guslipkin/jotto-cli" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /ttyd/ttyd /ttyd
COPY --from=build /ttyd/index.html /index.html
COPY --from=build /out/arcade /arcade
# Each session's throwaway save folder goes here (compose mounts a tmpfs over it).
COPY --from=build /out/tmp /tmp
ENV TERM=xterm-256color \
    HOME=/tmp
# No user database in a scratch image, so a bare unprivileged id.
USER 10001:10001
EXPOSE 7681
ENTRYPOINT ["/ttyd"]
# --writable: ttyd's terminals are read-only by default. --max-clients bounds
# how many play at once (each session runs its own menu and game); 0 is no limit.
CMD ["--writable", "--port", "7681", "--max-clients", "32", \
     "--index", "/index.html", \
     "--client-option", "titleFixed=Arcade", \
     "--client-option", "disableLeaveAlert=true", \
     "--client-option", "scrollback=0", \
     "/arcade", "--session-saves"]

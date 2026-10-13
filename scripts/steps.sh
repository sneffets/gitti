# Sourced by the bundle / install scripts: each step says what it does before it starts,
# with the time since the script began, so a long quiet phase (fat-LTO link) is never a
# mystery.
STEPS_START=$SECONDS

# `step "what happens now"`
step() {
  printf '\n==> [%dm%02ds] %s\n' $(((SECONDS - STEPS_START) / 60)) $(((SECONDS - STEPS_START) % 60)) "$*"
}

# Before a cargo build: what to expect from its output.
build_note() {
  case "$PROFILE" in
    release)
      echo "    cargo lists what it compiles; its last step (fat LTO, one codegen unit) links"
      echo "    the whole program in one go and prints nothing for several minutes."
      echo "    GITTI_BUILD_PROFILE=release-lowmem skips LTO: much faster, slightly slower binary."
      ;;
    release-lowmem)
      echo "    cargo lists what it compiles (no LTO: the last link takes seconds, not minutes)."
      ;;
  esac
}

# `done_steps "result line"`
done_steps() {
  step "done"
  echo "$*"
}

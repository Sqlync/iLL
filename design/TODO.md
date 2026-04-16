# TODO

## Current

"exec" actor examples — exec is for long-running processes (servers, daemons, brokers). Not intended for run-to-completion; side effects discouraged but not enforced.
- [ ] examples in examples folder
  - [ ] basic: long-running server + http_client smoke test (candidate for the simple README example, ~10 lines)
  - [ ] edge cases: startup timeout, process dies unexpectedly, env/args passing
- [ ] add a short simple example to README above the existing readme.ill deep-dive
- [ ] maybe update readme.ill to use exec instead of container for API


## Next

"exec" actor partial implementation
- [ ] integrate exec into completed phases (grammar if needed, validation rules, error shape `error.exec.reason`)


## Eventually

Update the image in README to use the latest readme.ill
# Changelog

All notable changes to ws63-examples will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **blinky** — LED blink example demonstrating GPIO output and busy-wait delay
  - Uses `ws63-rt::entry` for startup
  - Uses `ws63-hal::gpio::create_output_pin` for GPIO control
  - Demonstrates minimal `#![no_std]` + `#![no_main]` embedded application pattern

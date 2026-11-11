Third-party notices for Anada

manifest.json lists every third-party package in Anada's Windows build (the x86_64-pc-windows-gnu dependency graph) with its declared license and the notice files collected for it. Each crate directory under crates/ holds the license, copyright and attribution files that package publishes.

Some packages publish no license file of their own. For those, crates/<package>/reconstructed/ holds the canonical text of the declared license together with the package's authors from its Cargo.toml, clearly marked as reconstructed.

The default egui font notices are under crates/epaint_default_fonts-0.37.0/fonts. Rust standard-library notices are under rust. GNU runtime licensing, including the GCC Runtime Library Exception, is under rust-gnu-runtime and mingw.

These notices cover third-party components only. Anada itself is licensed under the GNU General Public License v3.0 or later. Virtual Audio Cable is a separate product under its vendor's license and is not included.

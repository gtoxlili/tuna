# Homebrew Formula for tuna.
#
# This file is the source-of-truth reference kept inside the tuna repo. The
# LIVE formula that `brew tap gtoxlili/tuna` serves lives in a separate
# gtoxlili/homebrew-tuna repo (Formula/tuna.rb), regenerated daily by that
# repo's sync-formula.yml workflow from the latest GitHub Release — so the
# version + macOS sha256 below are kept in sync automatically, no manual copy.
#
# Install (Homebrew 6.0+, which requires explicit tap trust). Use the fully-
# qualified name: homebrew-core has an unrelated `tuna` cask, so a bare
# `brew install tuna` would install that instead.
#   brew tap gtoxlili/tuna
#   brew trust gtoxlili/tuna
#   brew install gtoxlili/tuna/tuna
# Upgrade:
#   brew update && brew upgrade gtoxlili/tuna/tuna
class Tuna < Formula
  desc "Terminal tool for deriving 考研 English vocabulary by word roots"
  homepage "https://github.com/gtoxlili/tuna"
  license "GPL-3.0-or-later"
  version "0.1.25"

  # Pre-built binaries from GitHub Releases. Each archive unpacks to a single
  # top-level dir (tuna-v<ver>-<target>/) containing the `tuna` binary; Homebrew
  # auto-enters that dir during install, so `bin.install "tuna"` resolves.
  # The aarch64 build targets apple-m1 (runs on M1..M4); the x86_64 build is vanilla.
  on_macos do
    on_arm do
      url "https://github.com/gtoxlili/tuna/releases/download/v#{version}/tuna-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "d820b331d7b901a386f4f90cb4a8a0410f67614e88e6abe281538da011d5a927"
    end

    on_intel do
      url "https://github.com/gtoxlili/tuna/releases/download/v#{version}/tuna-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "3c2e2f88cd180f5b1daeeeb6bca85d70c17057325907e944ff3028fa86db66f9"
    end
  end

  # sherpa-onnx statically links its C++ runtime; the only dynamic deps are the
  # system CoreAudio frameworks, which ship with macOS. No brew dependencies.
  def install
    bin.install "tuna"
  end

  test do
    assert_match "tuna", shell_output("#{bin}/tuna --version")
  end
end

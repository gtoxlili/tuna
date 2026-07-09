# Homebrew Formula for tuna.
#
# This lives in the tuna repo as the source of truth. After each tagged release,
# copy it to the separate gtoxlili/homebrew-tuna repo (Formula/tuna.rb) with the
# real sha256 filled in — `shasum -a 256` the two macOS tarballs from the release.
#
# Install (Homebrew 6.0+, which requires explicit tap trust):
#   brew tap gtoxlili/tuna
#   brew trust gtoxlili/tuna
#   brew install tuna
# Upgrade:
#   brew update && brew upgrade tuna
class Tuna < Formula
  desc "Terminal tool for deriving 考研 English vocabulary by word roots"
  homepage "https://github.com/gtoxlili/tuna"
  license "GPL-3.0-or-later"
  version "0.1.0"

  # Pre-built binaries from GitHub Releases. Each archive unpacks to a single
  # top-level dir (tuna-v<ver>-<target>/) containing the `tuna` binary. The
  # aarch64 build targets apple-m1 (runs on M1..M4); the x86_64 build is vanilla.
  on_macos do
    on_arm do
      url "https://github.com/gtoxlili/tuna/releases/download/v#{version}/tuna-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_ARM64_SHA256"
    end

    on_intel do
      url "https://github.com/gtoxlili/tuna/releases/download/v#{version}/tuna-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_SHA256"
    end
  end

  # sherpa-onnx statically links its C++ runtime; the only dynamic deps are the
  # system CoreAudio frameworks, which ship with macOS. No brew dependencies.
  def install
    bin.install "tuna"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tuna --version")
  end
end

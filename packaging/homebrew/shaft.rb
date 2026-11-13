# Homebrew cask template. Fill in version/sha256 for each release and submit
# to a tap (e.g. nnevskij/homebrew-tap).
cask "shaft" do
  version "0.2.1"
  sha256 "f357c2219c335e0fc93af55a599c65e5c5aa7f6b9ae7eeff32a9e5d862831183"

  url "https://github.com/nnevskij/shaft/releases/download/v#{version}/Shaft-#{version}.dmg"
  name "Shaft"
  desc "Menu bar client for ngrok"
  homepage "https://github.com/nnevskij/shaft"

  depends_on macos: :ventura

  app "Shaft.app"

  zap trash: [
    "~/Library/Application Support/dev.shaft.app",
    "~/Library/Logs/dev.shaft.app",
  ]
end

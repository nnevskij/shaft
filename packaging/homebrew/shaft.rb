# Homebrew cask template. Fill in version/sha256 for each release and submit
# to a tap (e.g. nnevskij/homebrew-tap).
cask "shaft" do
  version "0.2.2"
  sha256 "ca843dfab254ed9b3fce77b65bedb34b4cbdbdfd639f54841bff95ff5f4e8c6b"

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

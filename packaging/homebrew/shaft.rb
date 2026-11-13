# Homebrew cask template. Fill in version/sha256 for each release and submit
# to a tap (e.g. nnevskij/homebrew-tap).
cask "shaft" do
  version "0.2.0"
  sha256 "REPLACE_WITH_DMG_SHA256"

  url "https://github.com/nnevskij/shaft/releases/download/v#{version}/Shaft-#{version}.dmg"
  name "Shaft"
  desc "Menu bar client for ngrok"
  homepage "https://github.com/nnevskij/shaft"

  depends_on macos: ">= :ventura"

  app "Shaft.app"

  zap trash: [
    "~/Library/Application Support/dev.shaft.app",
    "~/Library/Logs/dev.shaft.app",
  ]
end

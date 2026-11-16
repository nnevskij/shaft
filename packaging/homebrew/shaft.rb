# Homebrew cask template. Fill in version/sha256 for each release and submit
# to a tap (e.g. nnevskij/homebrew-tap).
cask "shaft" do
  version "0.3.1"
  sha256 "e5771c44cffa817403a32ad688259cad05bdb0e3b0d9aed075bbaa865b17a184"

  url "https://github.com/nnevskij/shaft/releases/download/v#{version}/Shaft-#{version}.dmg"
  name "Shaft"
  desc "Menu bar client for ngrok"
  homepage "https://github.com/nnevskij/shaft"

  auto_updates true
  depends_on macos: :ventura

  app "Shaft.app"

  zap trash: [
    "~/Library/Application Support/dev.shaft.app",
    "~/Library/Logs/dev.shaft.app",
  ]
end

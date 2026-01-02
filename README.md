# zed-hackatime

this is a fork of [zed-wakatime](https://github.com/wakatime/zed-wakatime) that should work better with [hackatime](https://hackatime.hackclub.com/)!

## Install

Search for "hackatime" in the "Extensions" page and click "Install".


### WakaTime configuration file
Create a file named `.wakatime.cfg` in your $HOME directory.
```toml
[settings]
api_key = your-api-key
```
Go through up [wakatime-cli](https://github.com/wakatime/wakatime-cli/blob/develop/USAGE.md)'s documentation for more options.

### LSP Settings

Multiple configuration options are available by editing your Zed settings file (`settings.json`). (The extension should work without any configuration, but you can customize it as needed.)
```json
"lsp": {
  "wakatime": {
    "initialization_options": {
      "api-key": "your-api-key",
      "api-url": "https://wakatime.com/api",
      "debug": false,
      "metrics": false,
      "heartbeat_interval": 120
    }
  }
}
```

#### Available options:
- `api-key` (string, required): Your WakaTime API key
- `api-url` (string, optional): Custom WakaTime API URL (e.g., for self-hosted instances)
- `debug` (boolean, optional): Enable debug logging (default: false)
- `metrics` (boolean, optional): Enable metrics collection (default: false)
- `heartbeat_interval` (integer, optional): Seconds between heartbeats for the same file (default: 120)

## Contributing

Don't hesitate to open an issue/submit a pr! this has been mainly tested on macos, but should work fine on other platforms as well.

const { defineConfig } = require('@playwright/test');
module.exports = defineConfig({
  testDir: './tests', fullyParallel: false, workers: 1, timeout: 30000,
  use: { baseURL:'http://127.0.0.1:8765', viewport:{width:1600,height:1000},
    launchOptions: { executablePath:process.env.CHROME_PATH || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', args:['--no-sandbox'] } },
  webServer:{command:'python3 -m http.server 8765 --bind 127.0.0.1', url:'http://127.0.0.1:8765', reuseExistingServer:true},
});

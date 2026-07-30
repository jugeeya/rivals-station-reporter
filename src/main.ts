import { createApp } from 'vue';
import App from './App.vue';
// Space Grotesk is the display face. Inter stays loaded behind it because
// player tags are user-entered and Space Grotesk's glyph coverage is much
// narrower, so anything outside its range still needs somewhere to land.
import '@fontsource-variable/space-grotesk';
import '@fontsource-variable/inter';
import '@fontsource-variable/ubuntu-sans-mono';
import './styles/global.scss';
import { install as installBrowserFixtures } from './dev/browserMock';

// No-op inside the desktop app; in a plain browser it stands in for the Rust
// side so the UI can be worked on with `pnpm dev` alone.
installBrowserFixtures();

createApp(App).mount('#app');

if (typeof WebSocket === 'undefined') {
  try {
    var WebSocket = require('websocket').w3cwebsocket;
  } catch(e) {
    throw Error('No WebSocket global, assuming this is running in node. "npm install websocket" for missing dependencies.');
  }
}

let downSocketUrl;

if (typeof location === 'undefined') {
  downSocketUrl = 'ws://localhost:3000/minidialer/socket';
} else {
  downSocketUrl = `ws://${location.host}/minidialer/socket`;
}

let numConns = 0;
let numIdleConns = 0;

function openConn() {
  numConns += 1;
  numIdleConns += 1;

  console.log(`new idle conn  ${numIdleConns} idle, ${numConns} total`);

  const downSocket = new WebSocket(downSocketUrl);
  // arraybuffer is significantly faster in chrome than default blob, tested
  // with chrome 123
  downSocket.binaryType = "arraybuffer";
  let upSocket = null;

  downSocket.onmessage = (e) => {
    console.log(`first byte     ${numIdleConns} idle, ${numConns} total`);
    numIdleConns -= 1;
    upSocket = new WebSocket(e.data);
    upSocket.binaryType = "arraybuffer";

    upSocket.onopen = () => {
      downSocket.send("ready");
    }

    upSocket.onmessage = (e) => {
      downSocket.send(e.data);
    }

    downSocket.onmessage = (e) => {
      upSocket.send(e.data);
    }

    upSocket.onerror = () => {
      upSocket.close();
    }

    upSocket.onclose = () => {
      downSocket.close();
    }
  }

  downSocket.onerror = () => {
    downSocket.close();
  }

  downSocket.onclose = () => {
    numConns -= 1;
    if (upSocket) {
      upSocket.close();
    } else {
      numIdleConns -= 1;
    }

    console.log(`conn closed    ${numIdleConns} idle, ${numConns} total`);
  }
}

setInterval(() => {
  if(numIdleConns < 1 && numConns < 200) {
    openConn();
  }
}, 1000);

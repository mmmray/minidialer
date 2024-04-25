if (typeof WebSocket === 'undefined') {
  try {
    var WebSocket = require('websocket').w3cwebsocket;
  } catch(e) {
    throw Error('No WebSocket global, assuming this is running in node. "npm install websocket" for missing dependencies.');
  }
}

let numConns = 0;
let numIdleConns = 0;

function openConn(csrfToken) {
  const url = `ws://${location.host}/minidialer/socket?csrf=${csrfToken}`;

  numConns += 1;
  numIdleConns += 1;

  console.log(`new idle conn  ${numIdleConns} idle, ${numConns} total`);

  const downSocket = new WebSocket(url);
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

function dialMain(csrfToken) {
  setInterval(() => {
    if(numIdleConns < 1 && numConns < 200) {
      openConn(csrfToken);
    }
  }, 1000);
}

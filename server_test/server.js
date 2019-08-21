const fs = require('fs');
const ipc = require('node-ipc');

ipc.config.id = 'wick';
ipc.config.retry = 5000;
ipc.config.networkHost = 'localhost';
ipc.config.networkPort = 27020;

let MessageHooks = [];

ipc.serveNet(() => {
    ipc.server.on('app.send_message', (data, socket) => {
        let hooks = MessageHooks.filter(v => v.type == data.type);
        Promise.all(hooks.map(v => Promise.resolve(v.callback(data.data)))).then(response => {
            let response_data = response;
            if (response.length === 1) {
                response_data = response[0];
            }
            ipc.server.emit(socket, 'app.receive_message', {
                data: response_data,
                request_id: data.request_id,
            });
        });
    });
});
ipc.server.start();

function BroadcastMessage(type, data) {
    ipc.server.broadcast('app.broadcast', {
        type: type,
        data: data,
    });
}

function AddMessageHook(type, callback) {
    MessageHooks.push({
        type: type,
        callback: callback,
    });
}

function GetFileName() {
    var now = new Date();
    var fileName = now.getFullYear() + '_' + now.getMonth() + '_' + now.getDate() + '.png';
    return fileName;
}

setInterval(() => {
    let fileName = GetFileName();
    BroadcastMessage("image", fileName);
}, 30000)

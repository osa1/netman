# A creatively named NetworkManager GUI

![Screenshot](https://github.com/user-attachments/assets/ca4177e1-0880-41d7-8ee3-b6d0213b11c2)

Note: this project is 100% vibe-coded. This document, however, is 100% written
by a human.

## Why?

I couldn't find a [NetworkManager] GUI that works on my system. After hours of
searching and trying projects that don't build, run, or work properly, I've
decided to give vibe-coding a try.

In about 15 minutes I had something working. A few more hours of prompting, and
it now does everything I need and nothing that I don't, using a stack that I
like. (Rust + [iced])

## Features

It works.

## Contributing

Please make sure to test these scenarios manually before sending patches:

- Errors are shown properly: e.g. try to connect a network with incorrect
  password.

- The GUI should update itself automatically: with device changes
  (added/removed), when you connect/disconnect via another method while the app
  is running (e.g. run `nmcli device disconnect <device id>`, `nmcli radio wifi
  off/on` in a shell while the app is running).

[NetworkManager]: https://www.networkmanager.dev/docs/api/latest/
[iced]: https://iced.rs/

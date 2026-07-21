#!/usr/bin/env node
/**
 * Agent Deck Simulator —— 终端虚拟键盘，连 host WS。
 */

import React from 'react'
import { render } from 'ink'
import { Deck } from './Deck.js'

const url = process.env.AGENT_DECK_URL ?? 'ws://127.0.0.1:8787'

render(React.createElement(Deck, { url }))

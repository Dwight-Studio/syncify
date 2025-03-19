/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
use std::time::Duration;

/// Simple finite state machine (FSM) to handle protocols.
pub trait FiniteStateMachine {
    type State;
    
    /// Execute a step.
    fn step(&mut self) -> impl Future<Output = ()> {
        async {
            match self.execute_step().await {
                Ok(state) => {
                    self.transition(state);
                }
                Err(e) => {
                    self.transition_error(e);
                }
            }
        }
    }

    /// Inner stepping execution.
    fn execute_step(&mut self) -> impl Future<Output = Result<Self::State, ProtocolError>>;
    
    /// Test if the FSM reached a final state.
    fn finished(&self) -> bool;
    
    /// Execute all steps until finished or timeout exceeded.
    /// 
    /// # Return
    /// 
    /// Returns true if finished, false if timed out.
    fn step_until_finished(&mut self, timeout: Duration) -> impl Future<Output = bool> {
        async move {
            tokio::time::timeout(timeout, async move {
                loop {
                    self.step().await;

                    if self.finished() {
                        break;
                    }
                }
            }).await.is_ok()
        }
    }
    
    /// Get the current state.
    fn current_state(&self) -> &Self::State;
    
    /// Get error if on failure state.
    fn error(&self) -> Option<ProtocolError>;

    /* Internal methods */
    
    /// Transition to state. Meant to be called by the FSM.
    fn transition(&mut self, state: Self::State);

    /// Transition to failure state. Meant to be called by the FSM.
    fn transition_error(&mut self, error: ProtocolError);
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ProtocolError {
    ConnectionFailed,
    ConnectionClosed,
    SendFailed,
    ReceiveFailed,
    Unexpected,
    InvalidSignature
}
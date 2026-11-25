use core::marker::PhantomData;
use core::task::Poll;

use embassy_usb_driver::EndpointError;
use futures_util::future::poll_fn;

use core::sync::atomic::Ordering;

use super::Instance;
use super::endpoint::{Endpoint, In, Out, TransferError};
use super::get_active_ep_state;
use crate::usb::{EP_IN_WAKERS, EP_OUT_WAKERS, IRQ_RESET, spin_until};

pub struct ControlPipe<'d, T: Instance> {
    pub(crate) _phantom: PhantomData<&'d mut T>,
    pub(crate) max_packet_size: usize,
    pub(crate) ep_in: Endpoint<'d, T, In>,
    pub(crate) ep_out: Endpoint<'d, T, Out>,
}

/// Direction of a control data/status stage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    In,
    Out,
}

impl<'d, T: Instance> ControlPipe<'d, T> {
    /// Wait for the primed ep0 stage to complete.
    ///
    /// Gives up with `Disabled` when the host ends the transfer another way — a bus reset, or a
    /// new SETUP packet (hosts abandon control transfers on timeouts, driver hand-over and
    /// re-enumeration). Without that the control task would wait here for good, with the bus
    /// events and every later SETUP unserved. What is still primed on ep0 is flushed, so it
    /// cannot go out as the answer to the next request.
    async fn wait_stage(&mut self, stage: Stage) -> Result<(), EndpointError> {
        let r = T::info().regs;
        let done = poll_fn(|cx| {
            match stage {
                Stage::In => EP_IN_WAKERS[0].register(cx.waker()),
                Stage::Out => EP_OUT_WAKERS[0].register(cx.waker()),
            }
            let complete = r.endptcomplete().read();
            if stage == Stage::In && complete.etce() & 1 != 0 {
                r.endptcomplete().write(|w| w.set_etce(1));
                Poll::Ready(true)
            } else if stage == Stage::Out && complete.erce() & 1 != 0 {
                r.endptcomplete().write(|w| w.set_erce(1));
                Poll::Ready(true)
            } else if r.endptsetupstat().read().endptsetupstat() & 1 != 0 || IRQ_RESET.load(Ordering::Acquire) {
                Poll::Ready(false)
            } else {
                Poll::Pending
            }
        })
        .await;
        if done {
            return Ok(());
        }
        self.flush();
        Err(EndpointError::Disabled)
    }

    /// Drop whatever is primed on ep0 (both directions).
    fn flush(&self) {
        let r = T::info().regs;
        r.endptflush().write(|w| {
            w.set_fetb(1);
            w.set_ferb(1);
        });
        spin_until(|| {
            let f = r.endptflush().read();
            (f.fetb() | f.ferb()) & 1 == 0
        });
    }
}

impl<'d, T: Instance> embassy_usb_driver::ControlPipe for ControlPipe<'d, T> {
    /// Maximum packet size for the control pipe
    fn max_packet_size(&self) -> usize {
        self.max_packet_size
    }

    /// Read a single setup packet from the endpoint.
    async fn setup(&mut self) -> [u8; 8] {
        let r = T::info().regs;

        // Clear interrupt status(by writing 1) and enable USB interrupt first
        r.usbsts().write(|w| w.set_ui(true));
        spin_until(|| !r.usbsts().read().ui());
        r.usbintr().modify(|w| w.set_ue(true));
        // Wait for SETUP packet
        let _ = poll_fn(|cx| {
            EP_OUT_WAKERS[0].register(cx.waker());
            if r.endptsetupstat().read().0 & 1 > 0 {
                // Clear the flag
                r.endptsetupstat().modify(|w| w.set_endptsetupstat(1));
                r.endptcomplete().write(|w| w.set_erce(1));
                // A stage of the previous request the host never collected (e.g. its status
                // stage) must not stay primed into this one
                self.flush();
                return Poll::Ready(Ok::<(), ()>(()));
            }

            Poll::Pending
        })
        .await;

        // Clear interrupt status and re-enable USB interrupt
        r.usbsts().write(|w| w.set_ui(true));
        spin_until(|| !r.usbsts().read().ui());
        r.usbintr().modify(|w| w.set_ue(true));

        // Read setup packet from qhd
        unsafe {
            let ep_state = get_active_ep_state();
            ep_state.qhd_list().qhd(0).get_setup_request()
        }
    }

    /// Read a DATA OUT packet into `buf` in response to a control write request.
    ///
    /// Must be called after `setup()` for requests with `direction` of `Out`
    /// and `length` greater than zero.
    async fn data_out(
        &mut self,
        buf: &mut [u8],
        _first: bool,
        _last: bool,
    ) -> Result<usize, embassy_usb_driver::EndpointError> {
        self.ep_out.transfer(buf).map_err(|_e| EndpointError::Disabled)?;
        self.wait_stage(Stage::Out).await?;
        Ok(buf.len())
    }

    /// Send a DATA IN packet with `data` in response to a control read request.
    ///
    /// If `last_packet` is true, the STATUS packet will be ACKed following the transfer of `data`.
    async fn data_in(
        &mut self,
        data: &[u8],
        _first: bool,
        last: bool,
    ) -> Result<(), embassy_usb_driver::EndpointError> {
        self.ep_in.transfer(data).map_err(|e| match e {
            TransferError::SetupPending => EndpointError::Disabled,
            _ => EndpointError::BufferOverflow,
        })?;
        self.wait_stage(Stage::In).await?;

        if last {
            // ZLT with empty buffer; fails only if the host has already moved on (new SETUP)
            let _ = self.ep_out.transfer(&[]);
        }
        Ok(())
    }

    /// Accept a control request.
    ///
    /// Causes the STATUS packet for the current request to be ACKed.
    async fn accept(&mut self) {
        // ZLT with empty buffer; fails only if the host has already moved on (new SETUP)
        if self.ep_in.transfer(&[]).is_ok() {
            let _ = self.wait_stage(Stage::In).await;
        }
    }

    /// Reject a control request.
    ///
    /// Sets a STALL condition on the pipe to indicate an error.
    async fn reject(&mut self) {
        // Reject, set IN+OUT to stall
        self.ep_in.set_stall();
        self.ep_out.set_stall();
    }

    /// Accept SET_ADDRESS control and change bus address.
    ///
    /// For most drivers this function should firstly call `accept()` and then change the bus address.
    /// However, there are peripherals (Synopsys USB OTG) that have reverse order.
    async fn accept_set_address(&mut self, addr: u8) {
        let r = T::info().regs;
        r.deviceaddr().modify(|w| {
            w.set_usbadr(addr);
            w.set_usbadra(true);
        });
        self.accept().await;
    }
}

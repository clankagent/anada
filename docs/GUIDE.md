# Anada user guide

Anada gives Windows four separate playback channels (Gaming, Chat, Media and Aux),
mixes them with their own EQ, and plays the result on the headphones or speakers you
listen on. You decide which app plays through which channel.

## Setup

1. Get Virtual Audio Cable (VAC) from the [official VAC site](https://vac.muzychenko.net/)
   and install it with its own installer. Anada's four channels are VAC lines. VAC is
   licensed separately by its vendor; the free trial adds a periodic voice reminder
   that a license removes.
2. Run `Anada-<version>-Setup.exe`. It installs Anada for your Windows user only and
   needs no administrator access or extra runtime. On the last page, clear
   **Start Anada now**.
3. Open **Start → Anada → Set up Virtual Audio Cable**. It configures the four stereo
   lines Anada uses, with endpoint volume enabled, and checks that each line has a
   playback and a recording device. It asks for administrator access once, keeps any
   extra cables you already have, and saves the previous settings; **Restore Virtual
   Audio Cable settings** puts them back. Both refuse to run while Anada is running.
4. Start **Anada** from the Start menu. Open **Settings → Windows audio** and click
   **Bind standard VAC Lines 1-4**. Each channel gets its VAC line's playback and
   recording device. You can change a binding there later; renamed cables are picked
   from the same menus.
5. Pick your headphones or speakers in the Output panel. On first start Anada lowers
   new devices to a safe level once, so nothing starts at full volume.
6. Click **Make Anada default** in **Settings → Windows audio** (or **Repair defaults**
   when the mixer shows that Windows defaults differ). Gaming becomes the normal
   Windows default and Chat becomes the communications default, so new audio goes
   through Anada.

| Anada channel | VAC line |
| --- | --- |
| Gaming | Line 1 (Virtual Audio Cable) |
| Chat | Line 2 (Virtual Audio Cable) |
| Media | Line 3 (Virtual Audio Cable) |
| Aux | Line 4 (Virtual Audio Cable) |

To update, run the new installer. Your settings are kept. If Anada is running, the
installer asks you to exit it first: choose **Exit host** in the tray menu or in **Settings → Startup & host**, then click
**Retry**.

## Mixer

Each channel strip shows its apps, a volume fader with signal meters, and **Mute** and
**Quiet** buttons. The faders are the Windows volume of each channel's device, so
Anada and Windows always show the same level. Click a percentage to type an exact
value; Enter or Tab applies it and Escape cancels.

Apps that play audio appear as pills. Drag a pill onto a channel, or click it and
choose a channel. With a pill focused, keys 1-4 send it to Gaming, Chat, Media or Aux.
Anada sets the Windows playback preference for every process of that app. Some apps
keep their current stream until playback restarts; their pill then says **Restart
app to move**. Apps that play straight to a real device appear under **Not in Anada**.

**Quiet** turns a channel or the output down to the quiet level and lights up. Click it
again to go back to the volume you had. It never turns anything up. Moving the fader
yourself ends Quiet at the new level, and switching the output device puts the old
device back to its earlier volume. Set the quiet level in **Settings → Channels**
(default 20%).

## Output

The Output panel below the channel strips holds the listening device. Choose it in
**Device**; **Priority** opens the priority list. Anada picks the highest connected
device on that list automatically and switches when devices connect or disconnect. A
device you pick by hand stays until the next connect or disconnect.

In **Settings → Outputs**, drag a device by its handle or name to reorder the list, or
focus the handle and press **Alt+Up** or **Alt+Down**. **Use now** switches to a device,
**×** removes it from the list, and **Add** puts an unlisted device at the end.
Disconnected devices keep their place and show in gray.

## Equalizer

Click a channel strip or its header to edit that channel's EQ below the mixer; click
**Output** for the output EQ. Output EQ is saved separately for each output device and
follows the device you choose.

Drag a point to change frequency and gain. Use the mouse wheel or Ctrl-drag for Q, and
hold Shift for fine steps. Exact values go in the number fields. Right-click a value
and choose **Reset**, or press **Ctrl+0**, to return it to its default. **Bypass**,
**Preamp**, **Reset** and **Presets** apply to the selected EQ. **EQ ±16 dB** changes
only the graph scale.

The spectrum behind the curve shows the live sound after EQ. The meters show digital
peaks in dBFS before Windows volume; a red mark holds briefly after clipping. If the
top bar says **Clipping**, lower the channel volume or the EQ preamp.

## Startup and the tray

Closing the window keeps audio running; Anada stays in the tray. The tray menu opens
the window, switches the listening output, and exits the host. **Start Anada at
sign-in** in **Settings → Startup & host** starts the host and tray when you log in. Anada then also appears in the Windows
**Startup apps** list (Windows Settings and Task Manager); turning it off there turns
it off in Anada too.

## Agent access (MCP)

Anada can let a local AI agent read or change selected settings for a limited time.
Open **Settings → Agent access (MCP)**, pick the permissions, choose a duration and
click **Enable session**. See [MCP access](MCP.md) for the permissions and setup.

## Restore normal sound

To bypass Anada, pick your real headphones or speakers in **Windows Settings → System
→ Sound**. In **Volume mixer**, set any app that uses a VAC line back to **Default**,
then restart its playback. Choose **Exit host** to stop Anada's mix.

## Uninstall

Choose **Exit host**, then use
**Windows Settings → Apps → Installed apps → Anada → Uninstall**. Your settings in
`%LOCALAPPDATA%\Anada` stay. VAC also stays, because other software may use it;
remove it with its own uninstaller once you have restored your Windows outputs.

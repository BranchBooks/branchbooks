# Android builds of the app (crates/branchbooks-ui): the dev image (tools/dev) plus a JDK, the Android
# SDK and NDK, the Rust Android targets and an emulator. Kept apart so the dev image does not carry
# several gigabytes of Android tools. CI does not build it yet.
# The Gradle cache and the emulator's virtual device live in volumes (compose.yaml); their folders
# are made here so the volumes take dev's ownership.
#   docker compose build dev && docker compose --profile android build android
#   docker compose run --rm android bash   (the emulator needs /dev/kvm, which compose passes in)

ARG BASE=branchbooks-dev
FROM ${BASE}

ARG CMDLINE_TOOLS=16111833
ARG NDK=29.0.14206865
ARG PLATFORM=android-35
ARG BUILD_TOOLS=35.0.1

USER root
ENV ANDROID_HOME=/opt/android ANDROID_SDK_ROOT=/opt/android \
    JAVA_HOME=/usr/lib/jvm/java-21-openjdk-amd64
ENV ANDROID_NDK_HOME=$ANDROID_HOME/ndk/$NDK NDK_HOME=$ANDROID_HOME/ndk/$NDK \
    PATH=$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$PATH

# The JDK for Gradle; unzip for the SDK; the libraries the emulator loads even with no window.
RUN apt-get update && apt-get install -y --no-install-recommends openjdk-21-jdk-headless unzip \
    libpulse0 libgl1 libxkbfile1 libasound2t64 libnss3 libxcomposite1 libxcursor1 libxi6 libxtst6 \
  && rm -rf /var/lib/apt/lists/* \
  && mkdir -p $ANDROID_HOME/cmdline-tools \
  && curl --proto '=https' --tlsv1.2 -fsSL -o /tmp/tools.zip \
    "https://dl.google.com/android/repository/commandlinetools-linux-${CMDLINE_TOOLS}_latest.zip" \
  && unzip -q /tmp/tools.zip -d /tmp && mv /tmp/cmdline-tools $ANDROID_HOME/cmdline-tools/latest \
  && rm /tmp/tools.zip \
  && yes | sdkmanager --licenses > /dev/null \
  && sdkmanager --install "platform-tools" "platforms;$PLATFORM" "build-tools;$BUILD_TOOLS" "ndk;$NDK" \
    "emulator" "system-images;$PLATFORM;google_apis;x86_64" > /dev/null \
  && rustup target add aarch64-linux-android x86_64-linux-android \
  && mkdir -p /home/dev/.gradle /home/dev/.android \
  && chown -R dev:dev $ANDROID_HOME /home/dev/.gradle /home/dev/.android

USER dev

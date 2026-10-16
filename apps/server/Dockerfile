FROM node:24

ENV NODE_ENV=production
ENV LOG_LEVEL=INFO
ENV CLIPROAM_ADMIN_PASSWORD=""

WORKDIR /app

COPY . .

RUN npm rebuild better-sqlite3

VOLUME ["/root/.cliproam"]

EXPOSE 4810

CMD ["node", "build/index.js"]
